//! 索引同步。数据库是事实来源，索引只是加速：查询结果一律回查数据库（SRCH-013），
//! 所以这里追求「最终一致、能自愈」，不追求不丢。
//!
//! 三条路径：
//! 1. 事件：总线上的 `post.published` / `post.removed` / `post.bumped` 与内部事件
//!    `comment.removed`、`user.renamed`，收集后每秒批量同步（低延迟）。
//! 2. 增量扫描：每 `sweep_secs` 扫一次最近改动的帖子（`updated_at`）与新评论（`created_at`），
//!    补上进程重启、事件丢失期间的变化。
//! 3. 全量对账：启动时与每 `reconcile_hours` 小时一次，用新的代次重写全部文档，
//!    再删除代次更旧的文档（库里已经没有的残留，EDGE-007）。
//!
//! 同步在业务事务提交之后、在独立任务里执行，从不阻塞写库事务（EDGE-015）。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ik_auth::service::Author;
use ik_comments::interop::CommentSearchDoc;
use ik_content::interop::PostSearchDoc;
use ik_kernel::bus::{Event, internal};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio::time::MissedTickBehavior;

use crate::engine::{CommentDocument, Engine, PostDocument};
use crate::text::markdown_to_plain;

const BATCH: i64 = 500;
/// 一次增量扫描最多处理的条数；更多的留给下一轮（水位只推进到处理过的位置）。
const SWEEP_CAP: i64 = 5000;
/// 帖子重新公开时补建的评论数上限。
const COMMENTS_PER_POST: i64 = 5000;
/// 增量扫描的水位往回多看一点，覆盖时钟误差与长事务晚提交。
const SWEEP_OVERLAP: chrono::Duration = chrono::Duration::seconds(10);
const FLUSH_EVERY: Duration = Duration::from_secs(1);
const GONE_USER_NAME: &str = "已注销用户";

pub(crate) struct Indexer {
    engine: Arc<Engine>,
    db: PgPool,
    /// 当前代次（毫秒时间戳）。增量写入也用它，所以全量对账期间写入的文档不会被收尾删除。
    generation: AtomicI64,
}

#[derive(Default)]
struct Dirty {
    posts: BTreeSet<i64>,
    /// 需要连带重建评论的帖子（发布、重新发布）。
    posts_with_comments: BTreeSet<i64>,
    removed_comments: BTreeSet<i64>,
    renamed: BTreeSet<i64>,
}

impl Dirty {
    fn is_empty(&self) -> bool {
        self.posts.is_empty()
            && self.posts_with_comments.is_empty()
            && self.removed_comments.is_empty()
            && self.renamed.is_empty()
    }

    fn absorb(&mut self, event: &Event) {
        let number = |key: &str| -> Option<i64> {
            let value = event.data.get(key)?;
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        };
        match event.kind {
            "post.published" => {
                if let Some(id) = number("postId") {
                    self.posts_with_comments.insert(id);
                }
            }
            "post.removed" | "post.bumped" => {
                if let Some(id) = number("postId") {
                    self.posts.insert(id);
                }
            }
            kind if kind == internal::COMMENT_REMOVED => {
                if let Some(id) = number("commentId") {
                    self.removed_comments.insert(id);
                }
            }
            kind if kind == internal::USER_RENAMED => {
                if let Some(id) = number("userId") {
                    self.renamed.insert(id);
                }
            }
            _ => {}
        }
    }
}

fn author_name(id: Option<i64>, authors: &HashMap<i64, Author>) -> Option<String> {
    let author = authors.get(&id?)?;
    Some(if author.gone {
        GONE_USER_NAME.to_owned()
    } else {
        author.username.clone()
    })
}

impl Indexer {
    pub(crate) fn new(engine: Arc<Engine>, db: PgPool) -> Self {
        Self {
            engine,
            db,
            generation: AtomicI64::new(Utc::now().timestamp_millis()),
        }
    }

    fn generation(&self) -> i64 {
        self.generation.load(Ordering::Relaxed)
    }

    async fn authors(
        &self,
        ids: impl Iterator<Item = i64>,
    ) -> anyhow::Result<HashMap<i64, Author>> {
        let mut ids: Vec<i64> = ids.collect();
        ids.sort_unstable();
        ids.dedup();
        ik_auth::service::authors(&self.db, &ids)
            .await
            .map_err(|p| anyhow::anyhow!("读取作者失败：{}", p.detail()))
    }

    async fn post_documents(&self, docs: Vec<PostSearchDoc>) -> anyhow::Result<Vec<PostDocument>> {
        let authors = self
            .authors(docs.iter().filter_map(|d| d.author_id))
            .await?;
        let generation = self.generation();
        Ok(docs
            .into_iter()
            .map(|d| PostDocument {
                id: d.id.to_string(),
                text: markdown_to_plain(&d.body),
                author_name: author_name(d.author_id, &authors),
                author_user_id: d.author_id.map(|id| id.to_string()),
                title: d.title,
                category_name: d.category_name,
                category_slug: d.category_slug,
                views: d.view_count,
                likes: d.like_count,
                comments: d.comment_count,
                bumped_at: d.bumped_at.timestamp_millis(),
                generation,
            })
            .collect())
    }

    async fn comment_documents(
        &self,
        docs: Vec<CommentSearchDoc>,
    ) -> anyhow::Result<Vec<CommentDocument>> {
        let authors = self
            .authors(docs.iter().filter_map(|d| d.author_id))
            .await?;
        let generation = self.generation();
        Ok(docs
            .into_iter()
            .map(|d| CommentDocument {
                id: d.id.to_string(),
                post_id: d.post_id.to_string(),
                parent_id: d.parent_id.map(|id| id.to_string()),
                content: crate::text::normalize_query(
                    Some(&d.text),
                    crate::text::INDEX_TEXT_MAX_CHARS,
                ),
                post_title: d.post_title,
                author_name: author_name(d.author_id, &authors),
                author_user_id: d.author_id.map(|id| id.to_string()),
                likes: d.like_count,
                created_at: d.created_at.timestamp_millis(),
                generation,
            })
            .collect())
    }

    /// 同步这些帖子：公开的写入，其余删除（连同它们的评论文档）。
    /// `with_comments` 时连带重建公开帖子的评论（发布、重新发布后评论要回到索引里）。
    pub(crate) async fn sync_posts(&self, ids: &[i64], with_comments: bool) -> anyhow::Result<()> {
        for chunk in ids.chunks(usize::try_from(BATCH).unwrap_or(500)) {
            let mut conn = self.db.acquire().await?;
            let docs = ik_content::interop::search_docs(&mut conn, chunk)
                .await
                .map_err(|p| anyhow::anyhow!("读取帖子失败：{}", p.detail()))?;
            drop(conn);
            let present: HashSet<i64> = docs.iter().map(|d| d.id).collect();
            let gone: Vec<i64> = chunk
                .iter()
                .copied()
                .filter(|id| !present.contains(id))
                .collect();
            let documents = self.post_documents(docs).await?;
            self.engine.upsert_posts(&documents).await?;
            self.engine.delete_posts(&gone).await?;
            self.engine.delete_comments_of_posts(&gone).await?;
            if with_comments {
                for post_id in present {
                    let mut conn = self.db.acquire().await?;
                    let comment_ids = ik_comments::interop::comment_ids_of_post(
                        &mut conn,
                        post_id,
                        COMMENTS_PER_POST,
                    )
                    .await
                    .map_err(|p| anyhow::anyhow!("读取评论失败：{}", p.detail()))?;
                    drop(conn);
                    self.sync_comments(&comment_ids).await?;
                }
            }
        }
        Ok(())
    }

    /// 同步这些评论：应当可见的写入，其余删除。
    pub(crate) async fn sync_comments(&self, ids: &[i64]) -> anyhow::Result<()> {
        for chunk in ids.chunks(usize::try_from(BATCH).unwrap_or(500)) {
            let mut conn = self.db.acquire().await?;
            let docs = ik_comments::interop::search_docs(&mut conn, chunk)
                .await
                .map_err(|p| anyhow::anyhow!("读取评论失败：{}", p.detail()))?;
            drop(conn);
            let present: HashSet<i64> = docs.iter().map(|d| d.id).collect();
            let gone: Vec<i64> = chunk
                .iter()
                .copied()
                .filter(|id| !present.contains(id))
                .collect();
            let documents = self.comment_documents(docs).await?;
            self.engine.upsert_comments(&documents).await?;
            self.engine.delete_comments(&gone).await?;
        }
        Ok(())
    }

    /// 评论被删除：删它和它的回复（回复已随一级评论级联删除）。
    pub(crate) async fn remove_comments(&self, ids: &[i64]) -> anyhow::Result<()> {
        self.engine.delete_comments(ids).await?;
        for id in ids {
            self.engine.delete_replies_of(*id).await?;
        }
        Ok(())
    }

    /// 改名或注销后更新作者名。
    pub(crate) async fn rename(&self, user_ids: &[i64]) -> anyhow::Result<()> {
        let authors = self.authors(user_ids.iter().copied()).await?;
        for id in user_ids {
            if let Some(name) = author_name(Some(*id), &authors) {
                self.engine.rename_author(*id, &name).await?;
            }
        }
        Ok(())
    }

    /// 全量对账：新代次重写全部文档，再删除旧代次的残留。
    pub(crate) async fn reconcile(&self) -> anyhow::Result<()> {
        let generation = Utc::now().timestamp_millis().max(self.generation() + 1);
        self.generation.store(generation, Ordering::Relaxed);
        let mut after = 0;
        let mut posts = 0;
        loop {
            let mut conn = self.db.acquire().await?;
            let docs = ik_content::interop::search_docs_after(&mut conn, after, BATCH)
                .await
                .map_err(|p| anyhow::anyhow!("读取帖子失败：{}", p.detail()))?;
            drop(conn);
            let Some(last) = docs.last().map(|d| d.id) else {
                break;
            };
            after = last;
            posts += docs.len();
            let documents = self.post_documents(docs).await?;
            self.engine.upsert_posts(&documents).await?;
        }
        let mut after = 0;
        let mut comments = 0;
        loop {
            let mut conn = self.db.acquire().await?;
            let (docs, last) = ik_comments::interop::search_docs_after(&mut conn, after, BATCH)
                .await
                .map_err(|p| anyhow::anyhow!("读取评论失败：{}", p.detail()))?;
            drop(conn);
            let Some(last) = last else {
                break;
            };
            after = last;
            comments += docs.len();
            let documents = self.comment_documents(docs).await?;
            self.engine.upsert_comments(&documents).await?;
        }
        self.engine.delete_older_than(generation).await?;
        tracing::info!(posts, comments, "搜索索引全量对账完成");
        Ok(())
    }

    /// 增量扫描：`since` 之后改动的帖子与新评论。返回下一轮的水位。
    pub(crate) async fn sweep(&self, since: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
        let started = Utc::now();
        let mut conn = self.db.acquire().await?;
        let posts = ik_content::interop::changed_post_ids(&mut conn, since, SWEEP_CAP)
            .await
            .map_err(|p| anyhow::anyhow!("读取改动的帖子失败：{}", p.detail()))?;
        let comments = ik_comments::interop::created_since(&mut conn, since, SWEEP_CAP)
            .await
            .map_err(|p| anyhow::anyhow!("读取新评论失败：{}", p.detail()))?;
        drop(conn);
        let capped =
            i64::try_from(posts.len().max(comments.len())).unwrap_or(i64::MAX) >= SWEEP_CAP;
        self.sync_posts(&posts, false).await?;
        self.sync_comments(&comments).await?;
        if capped {
            // 积压太多：水位不前进，下一轮接着扫（全量对账也会兜底）。
            tracing::warn!(cap = SWEEP_CAP, "搜索索引增量扫描触顶");
            return Ok(since);
        }
        Ok(started - SWEEP_OVERLAP)
    }

    async fn flush(&self, dirty: Dirty) -> anyhow::Result<()> {
        let with: Vec<i64> = dirty.posts_with_comments.iter().copied().collect();
        let plain: Vec<i64> = dirty
            .posts
            .difference(&dirty.posts_with_comments)
            .copied()
            .collect();
        self.sync_posts(&with, true).await?;
        self.sync_posts(&plain, false).await?;
        let removed: Vec<i64> = dirty.removed_comments.into_iter().collect();
        self.remove_comments(&removed).await?;
        let renamed: Vec<i64> = dirty.renamed.into_iter().collect();
        self.rename(&renamed).await
    }

    /// 后台循环，直到总线关闭。
    pub(crate) async fn run(
        self: Arc<Self>,
        mut events: broadcast::Receiver<Event>,
        sweep_every: Duration,
        reconcile_every: Duration,
    ) {
        // 搜索引擎可能比我们晚起来：设置推送失败就隔一会儿重试，期间查询走数据库兜底。
        let mut backoff = Duration::from_secs(5);
        while let Err(error) = self.engine.ensure().await {
            tracing::warn!(?error, "初始化搜索索引失败，稍后重试");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(300));
        }
        let mut watermark = Utc::now() - SWEEP_OVERLAP;
        if let Err(error) = self.reconcile().await {
            tracing::warn!(?error, "搜索索引全量对账失败");
        }

        let mut flush = tokio::time::interval(FLUSH_EVERY);
        flush.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut sweep =
            tokio::time::interval_at(tokio::time::Instant::now() + sweep_every, sweep_every);
        sweep.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut reconcile = tokio::time::interval_at(
            tokio::time::Instant::now() + reconcile_every,
            reconcile_every,
        );
        reconcile.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut dirty = Dirty::default();
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(event) => dirty.absorb(&event),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        // 丢掉的事件由增量扫描与全量对账补上。
                        tracing::warn!(skipped, "搜索索引同步跟不上事件总线");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = flush.tick(), if !dirty.is_empty() => {
                    if let Err(error) = self.flush(std::mem::take(&mut dirty)).await {
                        tracing::warn!(?error, "搜索索引同步失败");
                    }
                }
                _ = sweep.tick() => match self.sweep(watermark).await {
                    Ok(next) => watermark = next,
                    Err(error) => tracing::warn!(?error, "搜索索引增量扫描失败"),
                },
                _ = reconcile.tick() => {
                    if let Err(error) = self.reconcile().await {
                        tracing::warn!(?error, "搜索索引全量对账失败");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ik_kernel::bus::Audience;

    use super::*;

    fn event(kind: &'static str, data: serde_json::Value) -> Event {
        Event::new(kind, Audience::Internal, &data).unwrap()
    }

    #[test]
    fn events_mark_the_right_things_dirty() {
        let mut dirty = Dirty::default();
        dirty.absorb(&event(
            "post.published",
            serde_json::json!({ "postId": "7" }),
        ));
        dirty.absorb(&event("post.removed", serde_json::json!({ "postId": "8" })));
        dirty.absorb(&event("post.bumped", serde_json::json!({ "postId": "9" })));
        dirty.absorb(&event(
            internal::COMMENT_REMOVED,
            serde_json::json!({ "commentId": 3, "postId": 9 }),
        ));
        dirty.absorb(&event(
            internal::USER_RENAMED,
            serde_json::json!({ "userId": 4 }),
        ));
        dirty.absorb(&event(
            "notification.created",
            serde_json::json!({ "postId": "1" }),
        ));
        assert_eq!(dirty.posts_with_comments.iter().collect::<Vec<_>>(), [&7]);
        assert_eq!(dirty.posts.iter().collect::<Vec<_>>(), [&8, &9]);
        assert_eq!(dirty.removed_comments.iter().collect::<Vec<_>>(), [&3]);
        assert_eq!(dirty.renamed.iter().collect::<Vec<_>>(), [&4]);
    }
}
