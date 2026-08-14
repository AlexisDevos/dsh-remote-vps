# dsh-remote-vps

[English](README.md) | 中文 | [Français](README.fr.md)

面向 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 的**远程文件工具**：通过 SSH（推荐 Tailscale）操控**你自己的 VPS**，提供 `read`、`write`、`edit`、`read_image`、`glob`、`grep` 和 `bash`，服务器上**无需安装任何东西**。

```
[ 你的机器：DSH + 本包 ] ── SSH 多路复用（ControlMaster）──► [ VPS：零安装 ]
```

包内不包含任何个人信息：每位用户都在 **Settings → "VPS distants"** 中配置自己的服务器。

## 功能特性

- **完整的文件工具**：`read`/`write`/`edit`/`read_image` 保留 DSH 的原生语义（行号、原子写入、版本保护、类型化错误），但在 VPS 上执行。
- **远程搜索与 Shell**：`glob`/`grep`（ripgrep）和 `bash`（VPS Shell，如存在 nvm 则自动使用最新的 node）。
- **Settings 分区 "VPS distants"**：添加/编辑/删除连接、激活连接、测试（OK 徽章 + 延迟）、复制 SSH 命令、显示最后检测时间。
- **多连接**：在多个连接中激活一个，每个连接拥有各自的 `baseDir`（远程工作目录）。
- **VPS 零安装**：远程只用到 `sshd`、`python3` 和 `ripgrep`（大多数服务器上已预装）。
- **健壮可靠**：原子写入（temp + rename）、版本控制（`mtime ns:size`）、保留符号链接、保留 CRLF 换行符、类型化错误映射（`FS_PERMISSION_DENIED`、`FS_STALE_VERSION` 等）。

## 安装

### 1. 将本包安装到 web 配置文件目录

```bash
mkdir -p ~/.dsh/profiles/web/node_modules
cp -R dsh-remote-vps ~/.dsh/profiles/web/node_modules/dsh-remote-vps
```

### 2. 在配置文件补丁中声明根条目

编辑 `~/.dsh/profiles/web/cordis.patch.yml`：

```yaml
- insert:
    - id: dsh-remote-vps
      name: 'dsh-remote-vps'
```

该条目挂载根插件（SSH 连接池、持久化存储、本地 HTTP 桥、`glob`/`grep`/`bash` 工具、健康检查器）。它**不**提供 `fs` 服务：DSH 的本地文件系统在其他所有模式下保持原样。

### 3. 创建一个挂载远程文件系统的预设（preset）

复制 `standard` 预设（通过界面或 `~/.dsh/.agent-presets/`），移除 `tool-fs`、`tool-fs-search` 和 `tool-bash` 条目，然后加入以下分组：

```yaml
- id: remote
  name: cordis:group
  group: true
  isolate:
    fs: true
  config:
    - id: dsh-remote-vps-fs
      name: /dsh-remote-vps/的/绝对路径/src/fs.js

    - id: tool-fs
      name: '@deepseek-ai/dsh-tool-fs'

    - id: fs-observation-policy
      name: '@deepseek-ai/dsh-fs-observation-policy'
```

> 远程 `fs` 位于该预设每个会话私有的隔离域中：`standard`/`cordis`/`minimal` 模式仍保留各自受沙箱保护的本地文件系统。

### 4. 重启并配置

```bash
npx @deepseek-ai/dsh web
```

然后：**Settings → "VPS distants" → "+ 添加 VPS"**，填写：

| 字段 | 示例 | 必填 |
|---|---|---|
| 名称 | `我的 VPS` | 否 |
| 主机 | `my-vps`（Tailscale MagicDNS）或 `100.x.y.z` | **是** |
| 用户 | `root` | 否（默认 `root`） |
| 端口 | `22` | 否 |
| SSH 密钥 | `~/.ssh/id_ed25519` | 否 |
| baseDir | `/srv/app` | 否 |

连接持久化在 `~/.dsh/dsh-remote-vps.json` 中。**测试**按钮会显示实测延迟。

## 使用

预设激活后，工具即在 VPS 上工作：

- `read /home/user/app/README.md` — 带行号的远程读取；
- `write` / `edit` — 原子远程写入（带版本保护）；
- `glob "*.ts"` / `grep "workerAdapter"` — 远程 ripgrep（排除 `node_modules`/`.git`/`.next`/`dist`）；
- `bash` — VPS Shell（工作目录默认为 `baseDir`）。

未配置任何连接时，工具会返回明确的错误提示。

## 架构

| 文件 | 作用 |
|---|---|
| `src/index.js`（导出 `.`） | **根**插件：SSH 多路复用连接池（ControlMaster）、JSON 存储 `~/.dsh/dsh-remote-vps.json`、本地 HTTP 桥 `GET/POST /dsh-remote-vps/state`（严格限回环地址）、健康检查器、`glob`/`grep`/`bash` 工具。**不提供 `fs`。** |
| `src/fs.js`（导出 `./fs`） | `RemoteFileSystem` 类（即 `fs` 服务）— 在预设中挂载，与 `@deepseek-ai/dsh-tool-fs` 和 `@deepseek-ai/dsh-fs-observation-policy` 同置于 `isolate: { fs: true }` 分组中。 |
| `lib/client.js`（导出 `./client`） | 浏览器 bundle："VPS distants" Settings 分区（`package.json` 中的 `dsh.client` 声明）。 |
| `scripts/` | 测试与基准。 |

### 为什么用 HTTP 桥而不是 `ctx.settings`？

DSH 的设置通道（`settings.describe`/`settings.mutate`）只向浏览器客户端暴露一份**固定的命名空间白名单**；在当前 harness 版本中，第三方包无法自行注册。本地 `/dsh-remote-vps/state` 桥（仅回环地址、服务端 schema 校验）绕开了这一限制，**且不修改任何官方文件**。

## 安全性

- 不开放任何端口：出站 SSH 多路复用，使用你现有的密钥认证。
- HTTP 桥仅接受回环地址请求（`127.0.0.1`/`::1`）。
- `StrictHostKeyChecking=accept-new`：首次接触信任（经典 SSH 客户端的 TOFU 模型）。
- 不存储任何 API 密钥：本包只使用 SSH。
- 配合 Tailscale 时，VPS 的公网 IP 可以完全关闭 SSH：所有流量都走 tailnet。

## 测试

```bash
npm test            # 后端胶水层：write/edit（保护、符号链接、CRLF、权限）+ coreutils 命令 — 22 个场景
npm run test:client # 客户端 bundle：加载 + Settings 分区注册
npm run bench       # 针对存储中活动连接的操作延迟中位数
```

测试通过连接池的可注入传输在本地运行（无需 VPS）。仅 GNU 支持的命令（`find -printf`、`base64 -w0`）在 macOS BSD 上会跳过，并已在 Ubuntu VPS 上实测验证。

## 故障排查

| 现象 | 可能原因 |
|---|---|
| 分区中显示红色 "Pont local injoignable"（本地桥不可达） | 安装后未重启 DSH 服务器，或 `cordis.patch.yml` 无效 |
| "Écriture refusée: …"（写入被拒绝） | 消息会指出出错字段（缺少主机、host:port 重复、端口非数字） |
| Settings 中缺少该分区 | 重启 DSH 后强制刷新页面（Cmd+Shift+R） |
| 工具提示 "aucune connexion VPS configurée"（未配置 VPS 连接） | 在 Settings → "VPS distants" 中添加连接 |
| 测试显示 "Échec"（失败）徽章 | 检查主机密钥（`ssh-keyscan`）、SSH 密钥和 Tailscale 覆盖范围 |

## 许可证

MIT
