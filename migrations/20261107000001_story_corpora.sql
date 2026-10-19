-- 绝区零剧情库（ik-story）的语料版本。
--
-- 每行是一次上传的 zzz-story-extractor 产出（story.jsonl、supplement.jsonl、lore/*.md、manifest.json
-- 打成一个 gzip 压缩的 JSON 包），原样保存；索引不入库，各实例启动或切换版本时从包现场构建（约 1 秒），
-- 所以以后改了索引结构也不用迁移数据。同一时刻至多一行 active；只保留最近几个版本供回滚。

CREATE TABLE story_corpora (
    id              BIGINT PRIMARY KEY,
    -- manifest.json 里的数据版本（如 3.2.0）与语言，便于在后台辨认。
    source_version  TEXT NOT NULL,
    language        TEXT NOT NULL,
    -- 构建结果的统计：文档数、行数、主线说话人覆盖率（0–1）。
    docs            INTEGER NOT NULL CHECK (docs > 0),
    lines           INTEGER NOT NULL CHECK (lines > 0),
    speaker_coverage REAL NOT NULL CHECK (speaker_coverage BETWEEN 0 AND 1),
    -- 上传的 gzip 包与它的 SHA-256（十六进制），重复上传同一个包时可以认出来。
    bundle          BYTEA NOT NULL,
    bundle_sha256   TEXT NOT NULL CHECK (bundle_sha256 ~ '^[0-9a-f]{64}$'),
    active          BOOLEAN NOT NULL DEFAULT false,
    uploaded_by     BIGINT NOT NULL REFERENCES users (id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 至多一个生效版本；也是启动时查生效版本的索引。
CREATE UNIQUE INDEX story_corpora_active_uq ON story_corpora ((true)) WHERE active;
CREATE INDEX story_corpora_uploaded_by_idx ON story_corpora (uploaded_by);
CREATE INDEX story_corpora_created_idx ON story_corpora (created_at DESC, id DESC);
