-- 账号安全：修改密码签发的会话、米游社扫码注销核验。

-- 修改密码后给当前设备签的新会话（LOGIN-011，旧系统的 password_change）。
ALTER TABLE sessions DROP CONSTRAINT sessions_login_method_check;
ALTER TABLE sessions ADD CONSTRAINT sessions_login_method_check CHECK (
    login_method IN ('password', 'email_code', 'register', 'mihoyo', 'password_reset', 'password_change')
);

-- 扫码注销核验（MHY-016）：只核对扫码的米游社账号是不是当前账号绑定的那个，不建号、不改数据。
ALTER TABLE mihoyo_qr_logins DROP CONSTRAINT mihoyo_qr_logins_mode_check;
ALTER TABLE mihoyo_qr_logins ADD CONSTRAINT mihoyo_qr_logins_mode_check
    CHECK (mode IN ('login', 'bind', 'delete'));
ALTER TABLE mihoyo_qr_logins DROP CONSTRAINT mihoyo_qr_logins_user_check;
ALTER TABLE mihoyo_qr_logins ADD CONSTRAINT mihoyo_qr_logins_user_check
    CHECK ((mode IN ('bind', 'delete')) = (user_id IS NOT NULL));
