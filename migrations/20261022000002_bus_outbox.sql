-- 多实例事件总线（PostgreSQL LISTEN/NOTIFY）放不进 NOTIFY 载荷（约 8000 字节上限）的事件。
-- 通知里只带这里的 id，各实例收到后按 id 读取。只是转运，几分钟后由发布方删除。
-- 属于 ik_kernel::cluster，不属于任何业务域。

CREATE TABLE bus_outbox (
    id          BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payload     JSONB       NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX bus_outbox_created_at_idx ON bus_outbox (created_at);
