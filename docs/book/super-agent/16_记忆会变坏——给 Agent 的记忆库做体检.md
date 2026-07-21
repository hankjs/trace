# 记忆会变坏——给 Agent 的记忆库做体检

课程
Super Agent 实战课
记忆会变坏——给 Agent 的记忆库做体检
记忆会变坏——给 Agent 的记忆库做体检

约 20 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一节给 Agent 接上了 RAG，文档能查了、知识库能更新了。但你有没有想过一个问题——记忆系统呢？

跑了几个月之后，.memory/ 目录里堆了几十条记忆。其中有一条写着：

项目认证模块在 src/legacy/auth.ts，所有 JWT 签发和校验都走这个文件。

可你三个月前重构了认证模块，src/legacy/ 整个目录都删了。新的入口早就是 src/auth/index.ts 了。

你跟 Agent 说："帮我看看登录接口加个二次验证怎么实现。"

Agent 自信地查了下记忆库，命中了那条 legacy-auth-module，然后开始基于一个根本不存在的文件帮你写 patch——引用 src/legacy/auth.ts，跑起来直接报错。你去看它的输出，代码本身写得没毛病，逻辑也对，但它操作的文件压根不在了。你可能要翻半天才意识到——问题不在代码，在记忆。

说实话，这种情况比 Agent "不知道"要危险得多。不知道它会告诉你"我不确定"；但用了过期记忆，它表现得非常自信——生成的代码语法没问题、逻辑看着也合理，只是引用了一个不存在的文件。你可能要 debug 半小时才意识到问题出在记忆上。

这不是个例。Mem0 公开的数据是，三个月内大约三分之一的记忆会变得不准确。代码在变、配置在变、决策在变，记忆却像化石一样躺在那。

这一节我们来给记忆库装上完整的维护体系——源头控制、分级过期、定期体检、自动整理。

先装依赖：

bash
运行
复制
pnpm install

记忆为什么会坏

知识体系课里专门有一篇讲记忆失效模式的理论分析，这里快速过一遍，然后聚焦在工程实现上。其实就四种方式：

污染——把推测当事实存了。Agent 看到旧的 MySQL 配置文件就存下"项目用 MySQL"，实际上早迁到 PostgreSQL 了。

爆炸——只存不删，信噪比越来越低。500 条记忆里挑 5 条相关的，搜出来基本没用。

过期——代码变了记忆没跟上。这是最隐蔽也最危险的——Agent 不会怀疑自己的记忆，它会基于错误信息做出自信的错误决策。

冲突——新旧记忆互相矛盾。旧记忆说"不要写注释"，新记忆说"关键逻辑要加注释"。Agent 不知道信谁，行为会漂移。

这四种问题，我们接下来用三层手段分别应对：

不存清单解决污染——从源头控制别让垃圾进来；
lint + TTL 分级解决过期和爆炸——定期扫描，按类型判断该不该淘汰；
dream 自动整理解决冲突和残留——让 Agent 自己合并重复、清理垃圾。
不存清单——源头控制

对付污染最有效的方法，不是存完再检测，而是从一开始就别让垃圾进来。

你想想，Agent 最容易存错什么？那些能从其他地方推导出来的信息。代码里写着的、git 历史里记着的、配置文件里标明的——这些信息都有权威来源——代码本身就是，git 本身就是。Agent 再额外存一份到记忆里纯属多余。一旦原始数据变了（重构了、配置改了），记忆里那份副本就成了定时炸弹。

Claude Code 的记忆系统就有一份明确的"不存清单"：

代码能推导的不存：项目用什么语言、什么框架、目录结构——读代码就知道
git 能查的不存：谁改了什么、最近有哪些 PR——git log 是原始的数据源
文档已经写了的不存：CLAUDE.md 里的约定每次对话都会加载，重复存制造冲突
临时性的不存：当前任务上下文、debug 中的发现——会话结束就没用了

该存的：只存在于对话中、其他地方推导不出来的——用户偏好、纠正反馈、项目决策、外部资源引用。

这份原则写进 buildPromptSection，让模型自己过脑子：

src/memory/store.ts
应用
复制
buildPromptSection(): string {
  // ...
  const lines = [
    `[记忆系统] 共 ${entries.length} 条记忆`,
    '',
    '记忆索引：',
    index,
    '',
    '记忆使用原则：',
    '- 记忆是线索，不是事实——使用前先用工具验证（read_file、grep 确认）',
    '- 不存代码能推导的、git 能查的、文档已经写了的',
    '- 只存对话中出现的、其他地方推导不出来的信息',
  ];
  return lines.join('\n');
}


搜索也顺便从 includes 升级到了 BM25，代码在 src/memory/search.ts，原理跟 RAG 那节的全文检索一样，只是跑在内存里，不重复讲了。

lint 体检 + TTL 分级

不存清单解决的是"别让垃圾进来"，但已经进来的记忆怎么管？需要一个定期体检的机制——我们叫它 lint，跟代码里跑 eslint 一个意思：扫一遍，把有问题的报出来。

lint 检测三种问题：

记忆里引用的路径不存在了（stale_path）；
太久没被读过了（never_used）；
跟别的记忆重名了（duplicate_name）。

其中"太久没被读过"不能一刀切——你对 TypeScript 的偏好一年不读也不过期，但"下周五前不合代码"这种项目决策，一过周五就作废了。所以给不同类型设不同的"保质期"：

类型	TTL	理由
user（用户偏好）	365 天	"别给我写 Python"——基本不过期
feedback（纠正反馈）	90 天	"别用 mock 数据库"——三个月内有效
project（项目决策）	30 天	"移动端在切发布分支"——变化最快
reference（外部资源）	14 天	"Bug 在 xxx 项目"——链接会失效

在 validator.ts 里按 entry.type 查不同的阈值：

src/memory/validator.ts
应用
复制
const TTL_BY_TYPE: Record<string, number> = {
  user: 365,
  feedback: 90,
  project: 30,
  reference: 14,
};

export function validateEntry(entry: MemoryEntry, baseDir = '.'): ValidationIssue[] {
  const issues: ValidationIssue[] = [];

  // 路径过期检测
  const paths = extractPaths(entry.content);
  for (const p of paths) {
    const abs = path.isAbsolute(p) ? p : path.join(baseDir, p);
    if (!fs.existsSync(abs)) {
      issues.push({ kind: 'stale_path', message: `引用的路径不存在：${p}` });
    }
  }

  // 按类型 TTL 判断长期未用
  if (entry.lastReadAt) {
    const staleDays = TTL_BY_TYPE[entry.type] ?? 30;
    const days = (Date.now() - entry.lastReadAt) / (1000 * 60 * 60 * 24);
    if (days > staleDays) {
      issues.push({
        kind: 'never_used',
        message: `已 ${Math.floor(days)} 天没被读过，超过 ${entry.type} 类型的 ${staleDays} 天保质期`,
      });
    }
  }

  return issues;
}


TTL 判断需要知道"这条记忆最后一次被读是什么时候"，所以 store.ts 给每条记忆记录了 lastWriteAt 和 lastReadAt，每次 loadFile 时自动更新读时间。lintAll 把上面的 validateEntry 跑一遍全库，再额外检测重名——Agent 有时候会在不同时间点存两条名字相同但内容不一样的记忆。

给 memory tool 加上 lint action，再在快捷命令里加个 /lint，用户就能随时跑体检了。

bash
运行
复制
pnpm start


模板里预置了 5 条种子记忆，各种典型问题都有。输入 /lint 看效果：

text
复制
You: /lint

[lint] 记忆库 4 条有警告：
  📁 project_deploy-process-2.md  [project] deploy-process
     • duplicate_name: 存在 2 条同名记忆，可能需要合并
  📁 project_deploy-process.md  [project] deploy-process
     • stale_path: 引用的路径不存在：scripts/migrate.sh
     • stale_path: 引用的路径不存在：scripts/deploy.sh
     • duplicate_name: 存在 2 条同名记忆，可能需要合并
  📁 project_legacy-auth-module.md  [project] legacy-auth-module
     • stale_path: 引用的路径不存在：src/legacy/auth.ts
     • stale_path: 引用的路径不存在：src/middleware/auth-guard.ts
  📁 project_old-build-config.md  [project] old-build-config
     • stale_path: 引用的路径不存在：webpack.config.js
     • never_used: 已 854 天没被读过，超过 project 类型的 30 天保质期


typescript-preference 没出现——它是 user 类型，TTL 365 天，没引用路径。lint 精准地只报有问题的。

记忆自动整理——/dream

lint 能发现问题，但谁来解决呢？手动一条条改太累了。

Claude Code 的源码里有一套自动记忆整理机制，内部叫 Auto Dream（官网文档叫 Auto Memory）。它有两个触发条件——积累了 5 个以上的会话，并且距上次整理超过 24 小时，两个条件同时满足才会启动。

它的流程分四个阶段。先定位，看看当前有哪些记忆。然后检测，跑一遍 lint 把有问题的找出来。接着整理，根据报告删垃圾、合并重复的。最后裁剪，更新索引、输出一份报告说这次干了什么。

我们做个简化版——/dream 命令手动触发，基于之前的 lint 实现，让 Agent 用现有的 memory 工具自己完成四阶段：

src/index.ts
应用
复制
if (cmd === '/dream' || cmd === 'dream') {
  console.log('\n[dream] 开始记忆整理...');
  const dreamPrompt = [
    '请对记忆库做一次完整的整理（dream），按以下阶段执行：',
    '',
    '**阶段 1：定位** — 用 memory lint 扫描全库（结果已包含内容预览和问题清单，不需要逐条 read）。',
    '**阶段 2：整理** — 根据 lint 报告直接操作：',
    '  - 路径过期且长期未用的，直接 memory delete（传 filename）删掉',
    '  - 同名重复的，用 memory save 保存合并后的版本（同名自动覆盖），再 delete 多余的',
    '  - 内容仍然有效但描述不准确的，用 memory save 覆盖更新',
    '**阶段 3：报告** — 用一段文字总结这次整理做了什么。',
    '',
    '注意：read 和 delete 都需要传 filename（如 project_deploy-process.md），不是 name。',
  ].join('\n');

  const userMsg: ModelMessage = { role: 'user', content: dreamPrompt };
  messages.push(userMsg);
  store.append(userMsg);
  const currentSystem = builder.build(makePromptCtx());
  await agentLoop(model, registry, messages, currentSystem, tracker);
  console.log('  [dream 完成]\n');
  return true;
}


这个设计有意思的地方——整理逻辑不是硬编码的 if/else，而是交给模型自己判断。dream prompt 给了方向，具体哪条该删、哪条该留、怎么合并，全由模型根据 lint 报告决定。这也是 Claude Code 的思路——dream 本质是一个 prompt，不是一段写死的代码。

输入 /dream 试试。因为整理逻辑交给模型自己判断，每次跑的步数和顺序可能不同——模型可能先 list 再 lint，也可能先 read 每条记忆看内容再决定删谁。但最终效果是类似的：

text
复制
You: /dream

[dream] 开始记忆整理...

  Agent 先跑 memory list 看有哪些记忆
  再跑 memory lint 拿到体检报告
  逐条 read 有问题的记忆，判断该删还是该改
  调 memory delete 清理垃圾、合并重复
  最后输出一份报告

记忆整理完成！
- 删除了 old-build-config（854 天没读过，webpack.config.js 已不存在）
- 删除了 deploy-process-2（与 deploy-process 重名的早期废弃版本）
- 合并/更新了 deploy-process（保留部署流程，标注旧脚本路径已失效）
- 保留了 legacy-auth-module（路径过期但内容有参考价值）
- typescript-preference 健康，无需处理

记忆库从 5 条精简到 3 条。

  [dream 完成]


Agent 自己判断了该删什么、该留什么——old-build-config 这种又过期又没人读的果断删掉，legacy-auth-module 虽然路径过期但内容还有参考价值就先留着。真实模型跑的时候可能还会顺手把 deploy-process 里过期的脚本路径改掉，这都是模型根据 lint 报告自主决定的。

生产环境里可以把 /dream 接到 Cron 上——每天或每积累几个会话自动跑一次。Cron Job 机制在课程后续也会带大家一步步实现。

写在最后

回顾一下——四层防线形成闭环：

不存清单控制入口质量——别让垃圾进来。TTL 分级让不同记忆有不同保质期——偏好一年、决策一个月。lint定期扫描发现问题——路径过期的、超龄的、重名的全报出来。dream让 Agent 根据报告自己清理——不需要你手动一条条删。

说实话做记忆系统最容易踩的坑就是——只做了存，不做维护。用得越久越脏，搜出来噪音越多，最后 Agent 自己都不信自己的记忆了。记忆系统跟代码一样，需要持续维护。

下一节我们给 Agent 装上技能系统——让它能在不改核心代码的情况下按需扩展领域能力包。到那一步，Agent 就不只是个"啥都会一点"的通用助手了，而是能根据场景切换专业身份。

参考资料
Mem0 Production Best Practices — 记忆生命周期管理，"33% 记忆 90 天内变不准确"
Claude Code Memory — Claude Code 的 auto memory 机制
MINJA: Memory Injection Attacks — 记忆投毒攻击研究（成功率 98.2%）
OWASP Agent Security — ASI06 — 记忆污染安全风险
Letta (formerly MemGPT) — 记忆分层与生命周期开源实现
上一篇
RAG 实战——sqlite-vec + BM25 混合检索
下一篇 · 第五章：Skills + Plugins + Channel
Skills——给 Agent 注入领域知识
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
buildPromptSection(): string {
  // ...
  const lines = [
    `[记忆系统] 共 ${entries.length} 条记忆`,
    '',
    '记忆索引：',
    index,
    '',
    '记忆使用原则：',
    '- 记忆是线索，不是事实——使用前先用工具验证（read_file、grep 确认）',
    '- 不存代码能推导的、git 能查的、文档已经写了的',
    '- 只存对话中出现的、其他地方推导不出来的信息',
  ];
  return lines.join('\n');
}
```


```ts
const TTL_BY_TYPE: Record<string, number> = {
  user: 365,
  feedback: 90,
  project: 30,
  reference: 14,
};

export function validateEntry(entry: MemoryEntry, baseDir = '.'): ValidationIssue[] {
  const issues: ValidationIssue[] = [];

  // 路径过期检测
  const paths = extractPaths(entry.content);
  for (const p of paths) {
    const abs = path.isAbsolute(p) ? p : path.join(baseDir, p);
    if (!fs.existsSync(abs)) {
      issues.push({ kind: 'stale_path', message: `引用的路径不存在：${p}` });
    }
  }

  // 按类型 TTL 判断长期未用
  if (entry.lastReadAt) {
    const staleDays = TTL_BY_TYPE[entry.type] ?? 30;
    const days = (Date.now() - entry.lastReadAt) / (1000 * 60 * 60 * 24);
    if (days > staleDays) {
      issues.push({
        kind: 'never_used',
        message: `已 ${Math.floor(days)} 天没被读过，超过 ${entry.type} 类型的 ${staleDays} 天保质期`,
      });
    }
  }

  return issues;
}
```


```bash
pnpm start
```


```text
You: /lint

[lint] 记忆库 4 条有警告：
  📁 project_deploy-process-2.md  [project] deploy-process
     • duplicate_name: 存在 2 条同名记忆，可能需要合并
  📁 project_deploy-process.md  [project] deploy-process
     • stale_path: 引用的路径不存在：scripts/migrate.sh
     • stale_path: 引用的路径不存在：scripts/deploy.sh
     • duplicate_name: 存在 2 条同名记忆，可能需要合并
  📁 project_legacy-auth-module.md  [project] legacy-auth-module
     • stale_path: 引用的路径不存在：src/legacy/auth.ts
     • stale_path: 引用的路径不存在：src/middleware/auth-guard.ts
  📁 project_old-build-config.md  [project] old-build-config
     • stale_path: 引用的路径不存在：webpack.config.js
     • never_used: 已 854 天没被读过，超过 project 类型的 30 天保质期
```


```ts
if (cmd === '/dream' || cmd === 'dream') {
  console.log('\n[dream] 开始记忆整理...');
  const dreamPrompt = [
    '请对记忆库做一次完整的整理（dream），按以下阶段执行：',
    '',
    '**阶段 1：定位** — 用 memory lint 扫描全库（结果已包含内容预览和问题清单，不需要逐条 read）。',
    '**阶段 2：整理** — 根据 lint 报告直接操作：',
    '  - 路径过期且长期未用的，直接 memory delete（传 filename）删掉',
    '  - 同名重复的，用 memory save 保存合并后的版本（同名自动覆盖），再 delete 多余的',
    '  - 内容仍然有效但描述不准确的，用 memory save 覆盖更新',
    '**阶段 3：报告** — 用一段文字总结这次整理做了什么。',
    '',
    '注意：read 和 delete 都需要传 filename（如 project_deploy-process.md），不是 name。',
  ].join('\n');

  const userMsg: ModelMessage = { role: 'user', content: dreamPrompt };
  messages.push(userMsg);
  store.append(userMsg);
  const currentSystem = builder.build(makePromptCtx());
  await agentLoop(model, registry, messages, currentSystem, tracker);
  console.log('  [dream 完成]\n');
  return true;
}
```


```text
You: /dream

[dream] 开始记忆整理...

  Agent 先跑 memory list 看有哪些记忆
  再跑 memory lint 拿到体检报告
  逐条 read 有问题的记忆，判断该删还是该改
  调 memory delete 清理垃圾、合并重复
  最后输出一份报告

记忆整理完成！
- 删除了 old-build-config（854 天没读过，webpack.config.js 已不存在）
- 删除了 deploy-process-2（与 deploy-process 重名的早期废弃版本）
- 合并/更新了 deploy-process（保留部署流程，标注旧脚本路径已失效）
- 保留了 legacy-auth-module（路径过期但内容有参考价值）
- typescript-preference 健康，无需处理

记忆库从 5 条精简到 3 条。

  [dream 完成]
```
