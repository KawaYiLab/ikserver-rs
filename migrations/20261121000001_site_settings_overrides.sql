-- 后台可在线修改的站点参数（ik_kernel::site）：只存与部署配置缺省值不同的叶子，例如 {"hot":{"count":6}}。
-- 缺行或为空对象时全部取部署配置。
ALTER TABLE site_settings
    ADD COLUMN overrides JSONB NOT NULL DEFAULT '{}'::jsonb;
