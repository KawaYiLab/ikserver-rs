-- 站点设置：后台「系统设置」里可以随时切换、不必重启的开关。归 ik-auth（目前只有账号准入相关的开关）。
--
-- 只有一行（id 恒为 1）。读取方都按「缺行时取默认值」处理，所以这一行被误删也不会放开门控。
CREATE TABLE site_settings (
    id            SMALLINT    PRIMARY KEY DEFAULT 1,
    -- 入站考试开关：关闭后未通过考试的普通账号也能执行写操作，等级按经验计算（REG-007、EXP-002）。
    exam_required BOOLEAN     NOT NULL DEFAULT true,
    -- 最近一次在后台修改的时间；NULL = 一直是默认值。
    updated_at    TIMESTAMPTZ,

    CONSTRAINT site_settings_single_row CHECK (id = 1)
);
INSERT INTO site_settings (id) VALUES (1);
