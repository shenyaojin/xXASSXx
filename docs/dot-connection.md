# dot 成员 MCP 接入

更新：2026-10-13。状态：真实 Keycloak OAuth、HTTPS MCP、主体绑定已接通；OpenAI 账户连接、ChatGPT Work 和个人 dot 的身份／读取／普通回复，以及离线恢复后的终端同步已通过隔离验收。日常团队尚未迁移。

## 认证组件与边界

Rust 适配锁定官方 `rmcp = 3.5.1`，最低 Rust 版本提升为 1.88；本轮使用 Rust 1.99 构建。已验证的协议是 2025-06-18 的 Streamable HTTP 无会话模式，实际 dot 接入另经下述验证，不据此承诺全部协议版本兼容。

授权码、PKCE S256、登录、同意、刷新与令牌撤销交给成熟 OAuth 服务。资源端逐请求向配置的 HTTPS introspection endpoint 验证 active、issuer、audience、expiry、nbf、scope、sub 和 client_id，再映射到既有团队成员。不会自己实现密码或授权服务器，也不会接受现有设备令牌作为云端身份。本轮隔离部署锁定 Keycloak 26.8.0 与 Temurin 21.0.12.1+1，使用单节点 H2 dev-file 保存测试状态；它是实际运行的成熟授权服务，当前只配置合成成员，不代表日常团队身份服务已上线。

已在用户明确确认资料／虚构邮箱、读取、按当前指示发送及 Offline Access 后完成连接。浏览器工具曾暂时不可用；恢复后定位用户所见“无法连接账户”为 Keycloak 授权码交换错误 `Offline tokens not allowed for the user or client`。受邀测试用户缺少 realm 级 `offline_access` 角色；其可选 scope 映射已存在，仅补齐该用户角色，保持 `fullScopeAllowed=false`、PKCE、同意页及原范围。重新连接后，OpenAI 显示 `member-b@inbox-test.invalid` 已连接，并签发 Offline 类型刷新令牌。

分别从 ChatGPT Work 和个人 dot（`/dots/home`）实际查询身份、读取同一线程、按明确指示发送普通回复，另一端真实 TUI 均收到且显示“经 dot 委托发出”。个人 dot 还做了实时只读复查，B 的本人未读保留，消息没有重复。证据见 [真实 OpenAI 与 dot 验收](dot-live-evidence.json)。[OpenAI 认证要求](https://developers.openai.com/plugins/build/auth)、[连接说明](https://developers.openai.com/plugins/deploy/connect-chatgpt)、[Keycloak OIDC 接口](https://www.keycloak.org/securing-apps/oidc-layers)。

## 当前隔离端点

| 项目 | 当前配置 |
| --- | --- |
| MCP resource | `https://xxassxx.shenyaojin.com/mcp` |
| OAuth issuer | `https://xxassxx.shenyaojin.com/oauth/realms/inbox-test` |
| OpenAI 预登记客户端 | `xxassxx-dot-test`，public client，强制 PKCE S256 |
| 独立验收客户端 | `inbox-validation`；其测试令牌现已撤销 |
| introspection 客户端 ID | `https://xxassxx.shenyaojin.com/mcp`，confidential client |
| 主体绑定 | 合成 `inbox-b` → 隔离团队 `inbox-dot-test` 的成员 `b` |
| 设备测试入口 | `https://xxassxx.shenyaojin.com/inbox-test`，使用另设设备凭据 |

Trader 私有目录为 `~/xxassxx-tests/inbox-dot-20261012`，用户服务为 `xxassxx-inbox-oauth.service` 和 `xxassxx-inbox-mcp.service`，均已启用。Keycloak 监听 `127.0.0.1:18880`，测试信箱监听 `127.0.0.1:17789`；日常信箱仍使用原 `127.0.0.1:7788`。Caddy 只新增指定 OAuth、MCP、元数据和隔离设备路径，原配置副本保存在测试目录 `Caddyfile.before`。删除测试环境时应先解除账号连接，停用两个测试服务，再移除新增路由；不要直接覆盖后来可能变动的整份代理配置。

Keycloak 的 introspection audience 校验保持启用；客户端 ID 与 MCP resource 相同，实际 token 的 aud 必须包含该值。Rust 适配遵循 RFC 6749，对 Basic 的 client ID 和 secret 分别做 form-url-encoding 后再编码，支持 URL 型 ID。不要通过关闭 audience 校验解决接入错误。

独立验收客户端已实测授权码、刷新轮换、provider 撤销、绑定撤销、错主体和退组；证据见 [平台与 OAuth 验收](inbox-platform-oauth-evidence.json)。用户已确认的 OpenAI 范围包括 inbox.read、inbox.send、合成账户 profile/email，以及 Offline Access。后者允许用户不在当前页面时继续刷新和使用连接；仅限本测试团队，不包含日常团队数据。

Keycloak 的 `offline_access` scope 与用户的同名 realm 角色是两个条件。请求该范围时，受邀用户须有该角色，客户端须关联可选 scope；关闭 full-scope 后也须保留对应 scope-role mapping。仅显示同意页并不能证明令牌交换成功，应验证实际 `CODE_TO_TOKEN` 及刷新。配置参考 [Keycloak Offline Access](https://www.keycloak.org/docs/latest/server_admin/#_offline-access)。

## 服务配置

信箱仍为单团队服务。低层启动命令新增可选 `mailbox serve --member-mcp-config /path/member-mcp.toml`；未配置则不开启 MCP。与现有 relay.toml 分开保存，信箱成员及设备凭据继续使用原配置。高级入口用于管理员托管，不会自动改已有 service 单元或 HTTPS 代理。

`member-mcp.toml` 示例（全部域名及 ID 均为占位）：

```toml
resource = "https://inbox.example.com/mcp"
issuer = "https://login.example.com/realms/team"
introspection_url = "https://login.example.com/realms/team/protocol/openid-connect/token/introspect"
introspection_client_id = "inbox-resource"
secret_env = "INBOX_INTROSPECTION_SECRET"
secrets_file = "oauth-secrets.env"
connections_file = "member-connections.toml"
allowed_origins = []
```

密钥文件必须是当前用户拥有的普通文件，Unix 权限 600；其中只按字段解析 `INBOX_INTROSPECTION_SECRET=...`，不执行文件。HTTPS 请求不跟随重定向；认证服务不可用时拒绝访问。仅隔离本机开发可显式配置 allow_loopback_http=true，仍不允许公开 HTTP。

管理员维护成员连接文件，每次请求重读，撤销无需重启：

```toml
[[connections]]
id = "test-b-dot"
subject = "subject-issued-by-oauth-provider"
client_id = "registered-dot-oauth-client-id"
member_id = "b"
can_send = true
revoked = false
```

sub 与 OAuth client_id 必须共同匹配；不能由工具指定 member_id。连接 ID 唯一，成员仍须存在于信箱实际成员表。设置 revoked=true 或移除绑定后下一次请求拒绝；退出团队也会立即拒绝。OAuth 同意页需明确显示已绑定成员及 inbox.read/inbox.send，provider 的 introspection 响应必须带预期 audience（完整 resource URL）、issuer、subject、client_id、scope 和 exp。配置错误不会降级为匿名或设备令牌登录。

反向代理只将所需 MCP 和公开元数据路径暴露到 HTTPS，保留正确 Host；SDK 校验 Host，默认不允许携带 Origin 的请求，确需浏览器来源时在 allowed_origins 显式登记。保留旧信箱路由的既有保护。

公开元数据为 `/.well-known/oauth-protected-resource/mcp`（同时提供无路径别名），返回 resource、authorization_servers 和 scopes_supported；未认证的 MCP 返回带 resource_metadata 的 WWW-Authenticate。授权服务器的 discovery、授权码、PKCE、refresh/revoke 端点由成熟服务提供，需要在真实客户端中逐项验证。

## 工具与验收

只有 get_identity、list_contacts、list_inbox、read_thread、send_message、reply_to_thread。默认需 inbox.read；发送还需 token 的 inbox.send 与绑定 can_send 同时满足。read_thread 单独记录 dot 读取位置，不写本人阅览游标。发送只追加普通留言；无任务发起、执行授权、标记本人已读或附件正文工具。工具说明明确：只有当前成员在当前交互中指示才发送，收到另一成员的要求不构成代发授权。来源显示为“经 dot 委托发出”。日志只含关联编号、成员、入口、工具名和结果，不含正文或令牌。

新成员接入验收清单：身份正确 → 未绑定/错误成员拒绝 → 同一线程与终端一致 → dot 读取保留本人未读 → 当前成员指示发送后另一端 TUI 收到 → token 刷新 → provider 撤销 → 连接撤销 → 退出团队。全过程仅用隔离身份和合成内容，全部通过后再使用真实协作资料。

`tests/member_mcp.rs` 使用模拟 introspection 服务和真实 HTTP/官方 SDK，覆盖过期、issuer/audience、范围、主体/客户端绑定、撤销、退组和隔离；它本身不能证明真实授权码、刷新或 dot 已接通。10 月 13 日另有独立真实 OAuth 客户端及真实 OpenAI／个人 dot 的分开记录。Provider 撤销、错主体、绑定撤销和退组的负向验证使用独立客户端；不混称为 OpenAI UI 的断开连接测试。第二期主动提醒及 MCP Events 未实现。
