//! 单条连接的生命周期：收客户端帧、转发事件、心跳、会话复查、关闭。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use futures_util::SinkExt;
use ik_auth::{AuthState, Principal};
use tokio::time::{Instant, MissedTickBehavior};

use crate::hub::{Hub, Outbound, Registration, SubscribeError};
use crate::protocol::{
    CLOSE_IDLE, CLOSE_SESSION_ENDED, ClientFrame, CloseReason, HEARTBEAT_SECS, HelloData,
    MAX_FRAME_BYTES, ServerFrame, channel_of,
};

/// 已登录连接定期回数据库复查会话（吊销事件丢失时的兜底，WS-008）。
const RECHECK_EVERY: Duration = Duration::from_secs(30);
/// 单次写超时：客户端长时间不读时不让连接任务卡死。
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// 复查会话的超时；超时按失效处理（fail-closed）。
const RECHECK_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct Connection {
    pub hub: Arc<Hub>,
    pub auth: AuthState,
    pub principal: Option<Principal>,
    pub registration: Registration,
}

enum Flow {
    Continue,
    Close(CloseReason),
    /// 客户端发来了关闭帧：底层已排好关闭回复，只需冲刷出去。
    PeerClosed,
    /// 对端已断开或写失败，不再发关闭帧。
    Gone,
}

async fn send_text(socket: &mut WebSocket, text: Utf8Bytes) -> Flow {
    match tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Text(text))).await {
        Ok(Ok(())) => Flow::Continue,
        _ => Flow::Gone,
    }
}

async fn send_frame(socket: &mut WebSocket, frame: &ServerFrame) -> Flow {
    match serde_json::to_string(frame) {
        Ok(text) => send_text(socket, text.into()).await,
        Err(error) => {
            tracing::error!(%error, "序列化服务端帧失败");
            Flow::Continue
        }
    }
}

async fn session_still_active(auth: &AuthState, principal: &Principal) -> bool {
    let check =
        ik_auth::service::session_active(auth, principal.user_id.get(), principal.session_id.get());
    match tokio::time::timeout(RECHECK_TIMEOUT, check).await {
        Ok(Ok(active)) => active,
        Ok(Err(error)) => {
            tracing::warn!(?error, "复查会话失败，断开连接");
            false
        }
        Err(_) => {
            tracing::warn!("复查会话超时，断开连接");
            false
        }
    }
}

impl Connection {
    pub(crate) async fn run(mut self, mut socket: WebSocket) {
        let id = self.registration.id;
        match self.serve(&mut socket).await {
            Flow::Close(reason) => {
                let frame = CloseFrame {
                    code: reason.code,
                    reason: reason.reason.into(),
                };
                let _ =
                    tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Close(Some(frame))))
                        .await;
            }
            // 不冲刷的话客户端会看到「对方没有完成关闭握手」。
            Flow::PeerClosed => {
                let _ = tokio::time::timeout(WRITE_TIMEOUT, socket.flush()).await;
            }
            Flow::Continue | Flow::Gone => {}
        }
        self.hub.unregister(id);
    }

    /// 运行到连接需要结束为止，返回结束方式（不会是 Continue）。
    async fn serve(&mut self, socket: &mut WebSocket) -> Flow {
        let hello = ServerFrame::Hello {
            d: HelloData {
                heartbeat_sec: HEARTBEAT_SECS,
                user_id: self.principal.as_ref().map(|p| p.user_id.to_string()),
                server_time: chrono::Utc::now(),
            },
        };
        if let Flow::Gone = send_frame(socket, &hello).await {
            return Flow::Gone;
        }

        let heartbeat = Duration::from_secs(HEARTBEAT_SECS);
        let mut last_seen = Instant::now();
        let mut ticker = tokio::time::interval_at(Instant::now() + heartbeat, heartbeat);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut recheck = tokio::time::interval_at(Instant::now() + RECHECK_EVERY, RECHECK_EVERY);
        recheck.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let flow = tokio::select! {
                incoming = socket.recv() => match incoming {
                    Some(Ok(message)) => {
                        last_seen = Instant::now();
                        self.on_message(socket, message).await
                    }
                    // 帧超过上限、协议错误或连接断开。
                    Some(Err(_)) | None => Flow::Gone,
                },
                outbound = self.registration.outbound.recv() => match outbound {
                    Some(Outbound::Frame(text)) => send_text(socket, Utf8Bytes::from(&*text)).await,
                    Some(Outbound::Recheck) => self.recheck().await,
                    None => Flow::Gone,
                },
                changed = self.registration.close.changed() => match changed {
                    Ok(()) => match *self.registration.close.borrow() {
                        Some(reason) => Flow::Close(reason),
                        None => Flow::Continue,
                    },
                    Err(_) => Flow::Gone,
                },
                _ = ticker.tick() => {
                    let idle = last_seen.elapsed();
                    if idle >= heartbeat * 2 {
                        Flow::Close(CLOSE_IDLE)
                    } else if idle >= heartbeat {
                        match tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Ping(Default::default()))).await {
                            Ok(Ok(())) => Flow::Continue,
                            _ => Flow::Gone,
                        }
                    } else {
                        Flow::Continue
                    }
                },
                _ = recheck.tick(), if self.principal.is_some() => self.recheck().await,
            };
            match flow {
                Flow::Continue => {}
                end => return end,
            }
        }
    }

    async fn recheck(&self) -> Flow {
        match &self.principal {
            Some(principal) if !session_still_active(&self.auth, principal).await => {
                Flow::Close(CLOSE_SESSION_ENDED)
            }
            _ => Flow::Continue,
        }
    }

    async fn on_message(&self, socket: &mut WebSocket, message: Message) -> Flow {
        let text = match message {
            Message::Text(text) => text,
            Message::Close(_) => return Flow::PeerClosed,
            // ping 由底层自动回 pong；二进制帧不在协议里，忽略。
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) => return Flow::Continue,
        };
        if text.len() > MAX_FRAME_BYTES {
            return send_frame(
                socket,
                &ServerFrame::error(None, "INVALID_FRAME", "消息过大"),
            )
            .await;
        }
        let Ok(frame) = serde_json::from_str::<ClientFrame>(text.as_str()) else {
            return send_frame(
                socket,
                &ServerFrame::error(None, "INVALID_FRAME", "不是合法的 JSON 帧"),
            )
            .await;
        };
        let seq = frame.seq;
        let reply = match frame.op.as_str() {
            "ping" => seq.map(|seq| ServerFrame::Ack { seq }),
            "subscribe" => match channel_of(frame.t.as_deref(), frame.d.as_ref()) {
                None => Some(ServerFrame::error(
                    seq,
                    "CHANNEL_INVALID",
                    "频道不存在或无权订阅",
                )),
                Some(channel) => match self.hub.subscribe(self.registration.id, channel) {
                    Ok(()) => seq.map(|seq| ServerFrame::Ack { seq }),
                    Err(SubscribeError::TooMany) => Some(ServerFrame::error(
                        seq,
                        "TOO_MANY_SUBSCRIPTIONS",
                        "订阅的频道过多",
                    )),
                },
            },
            "unsubscribe" => {
                if let Some(channel) = channel_of(frame.t.as_deref(), frame.d.as_ref()) {
                    self.hub.unsubscribe(self.registration.id, &channel);
                }
                seq.map(|seq| ServerFrame::Ack { seq })
            }
            // 私信模块上线后支持。
            "typing" => Some(ServerFrame::error(seq, "UNSUPPORTED", "暂不支持该操作")),
            _ => Some(ServerFrame::error(seq, "UNKNOWN_OP", "未知的操作")),
        };
        match reply {
            Some(frame) => send_frame(socket, &frame).await,
            None => Flow::Continue,
        }
    }
}
