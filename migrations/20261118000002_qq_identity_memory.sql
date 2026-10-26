-- QQ 授权快照在同会话首次执行时冻结，不倒推平台发送瞬间的绑定。
ALTER TABLE qq_identities ADD COLUMN binding_version BIGINT NOT NULL DEFAULT 1 CHECK (binding_version > 0);
ALTER TABLE qq_messages ADD COLUMN owner_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE qq_messages ADD COLUMN binding_version BIGINT CHECK (binding_version > 0);
CREATE INDEX qq_messages_owner ON qq_messages(owner_user_id) WHERE owner_user_id IS NOT NULL;

-- 以已经结束、确认内容已写完的 turn 为完整提炼单元。时间仅排序，processed 才是去重依据。
ALTER TABLE qq_turns ADD COLUMN personal_memory_processed_at TIMESTAMPTZ;
ALTER TABLE qq_turns ADD COLUMN group_memory_processed_at TIMESTAMPTZ;
UPDATE qq_turns SET personal_memory_processed_at = now(), group_memory_processed_at = now();
CREATE INDEX qq_turns_personal_memory_pending ON qq_turns(payer_user_id, ai_user_id, finished_at, id)
    WHERE personal_memory_processed_at IS NULL AND finished_at IS NOT NULL;
CREATE INDEX qq_turns_group_memory_pending ON qq_turns(group_id, ai_user_id, finished_at, id)
    WHERE group_memory_processed_at IS NULL AND finished_at IS NOT NULL;

-- 旧记录没有稳定授权归属：不补 owner，不自动重新提炼，不注入新的单聊上下文。
UPDATE qq_identities SET context_reset_at = GREATEST(context_reset_at, now()) WHERE scene = 'c2c';
