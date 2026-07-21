# 权限系统 + Hook 管线——让 Agent 安全地被别人使用

课程
Super Agent 实战课
权限系统 + Hook 管线——让 Agent 安全地被别人使用
权限系统 + Hook 管线——让 Agent 安全地被别人使用

约 38 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一节我们把 Agent 接入了飞书群，你不再是唯一使用 Agent 的人了。群里任何人都能 @Bot 说一句"帮我跑个 rm -rf"，Agent 如果老老实实执行了，那就出大事了。

自己用的时候权限不是问题——你信任自己。但一旦把 Agent 暴露给其他人，"谁能做什么"就变成必须解决的事情。另外，工具执行前后能不能插入自定义逻辑——比如记审计日志、做格式检查——这在生产环境里同样重要。

这一节我们做三件事：角色权限控制谁能用什么工具、Bash 风险检测拦截危险命令、Hook 管线在工具执行前后插入自定义逻辑。

先装依赖：

bash
运行
复制
pnpm install

三级角色权限

权限模型不需要特别复杂，对于 Agent 场景，三个角色就够了：

owner：能用所有工具，包括 bash
collaborator：能用大部分工具，但 bash 被禁（因为 bash 能干的事太多了）
guest：只能用安全的只读工具——查天气、读文件、搜索

新建 src/security/roles.ts：

src/security/roles.ts
应用
复制
export type Role = 'owner' | 'collaborator' | 'guest';

export interface UserIdentity {
  id: string;
  name: string;
  role: Role;
}

const TOOL_ACCESS: Record<Role, { allow: string[] | '*'; deny: string[] }> = {
  owner: {
    allow: '*',
    deny: [],
  },
  collaborator: {
    allow: '*',
    deny: ['bash'],
  },
  guest: {
    allow: ['get_weather', 'calculator', 'read_file', 'list_directory', 'glob', 'grep', 'rag_search'],
    deny: [],
  },
};

export function canUseTool(role: Role, toolName: string): boolean {
  const access = TOOL_ACCESS[role];
  if (access.deny.includes(toolName)) return false;
  if (access.allow === '*') return true;
  return access.allow.includes(toolName);
}

export function filterToolsForRole(toolNames: string[], role: Role): string[] {
  return toolNames.filter(name => canUseTool(role, name));
}


设计上用了白名单 + 黑名单双重过滤。owner 是 allow: '*'（全部允许），collaborator 也是 allow: '*' 但 deny: ['bash']（全允许但禁 bash），guest 是显式白名单。这样新增工具时 owner 和 collaborator 自动能用，guest 需要手动加——默认安全。

实际上，Channel 那节讲的 Gateway 已经有 senderId 了，未来完全可以把角色分配跟 Channel 打通——终端里默认 owner，飞书群里根据发送者的 open_id 查角色表分配。这就是为什么我们在设计 IncomingMessage 时保留了 senderId 字段。

然后在 ToolRegistry 的 getActiveTools 里加一行过滤：

src/tools/registry.ts
应用
复制
getActiveTools(): ToolDefinition[] {
  return this.getAll().filter(tool => {
    // ...原有的 profile 和 defer 过滤...
    if (!canUseTool(this.currentRole, tool.name)) {
      return false;  // ← 角色不允许的工具直接不暴露给模型
    }
    return true;
  });
}


被过滤掉的工具不会出现在模型的 tools 列表里——模型根本不知道有这个工具存在，自然也不会去调用它。这比"让模型调用后再报错"安全得多，因为模型看到工具列表里有 bash，即使你说"不要用"，它有时候还是会试。

ToolRegistry 本身也需要暴露几个方法来支撑角色系统：setRole() 切换当前角色、getRole() 读取当前角色。默认角色是 owner——在终端里自己用，天然是最高权限。

有了角色数据和过滤逻辑，还缺一个让用户交互式切换角色的入口。新建 src/commands/security.ts，把 /role 和 /hooks 两个命令放在一起：

src/commands/security.ts
应用
复制
import type { CommandHandler } from './index.js';
import type { ToolRegistry } from '../tools/registry.js';
import type { HookPipeline } from '../security/hooks.js';
import type { Role } from '../security/roles.js';

export function createSecurityCommands(
  registry: ToolRegistry,
  hookPipeline: HookPipeline,
): CommandHandler[] {
  return [
    // /role [owner|collaborator|guest]
    (cmd, _ctx) => {
      const match = cmd.match(/^\/role(?:\s+(owner|collaborator|guest))?$/);
      if (!match) return false;

      if (match[1]) {
        const role = match[1] as Role;
        registry.setRole(role);
        const toolCount = registry.getActiveTools().length;
        console.log(`\n[security] 角色切换为 ${role}，可用工具: ${toolCount} 个\n`);
      } else {
        const role = registry.getRole();
        const toolCount = registry.getActiveTools().length;
        console.log(`\n[security] 当前角色: ${role}，可用工具: ${toolCount} 个\n`);
      }
      return true;
    },

    // /hooks
    (cmd, _ctx) => {
      if (cmd !== '/hooks') return false;

      const hooks = hookPipeline.list();
      console.log('\n[hooks]');
      if (hooks.pre.length > 0) {
        console.log('  Pre-Tool Hooks:');
        for (const name of hooks.pre) console.log(`    - ${name}`);
      }
      if (hooks.post.length > 0) {
        console.log('  Post-Tool Hooks:');
        for (const name of hooks.post) console.log(`    - ${name}`);
      }
      if (hooks.pre.length === 0 && hooks.post.length === 0) {
        console.log('  没有注册的 Hook');
      }
      console.log('');
      return true;
    },
  ];
}


createSecurityCommands 接收 registry 和 hookPipeline 两个依赖。/role 不带参数时显示当前角色和可用工具数；带参数时调用 registry.setRole() 切换角色，切换后立刻算一次 getActiveTools().length，让用户直观看到工具数量的变化。/hooks 调用 hookPipeline.list() 列出所有已注册的 hook 名称——后面我们实现 HookPipeline 的时候会加上这个 list() 方法。

bash
运行
复制
pnpm start


试试切换角色：

text
复制
You: /role
[security] 当前角色: owner，可用工具: 16 个

You: /role guest
[security] 角色切换为 guest，可用工具: 7 个

You: /role owner
[security] 角色切换为 owner，可用工具: 16 个


guest 模式下工具从 16 个骤降到 7 个——bash、write_file、edit_file 这些危险工具全没了。

Bash 命令风险检测

即使是 owner 角色，有些 bash 命令也不该执行。rm -rf /、sudo、curl xxx | sh——这些要么是误操作，要么是 prompt injection 诱导模型干的。

新建 src/security/bash-classifier.ts：

src/security/bash-classifier.ts
应用
复制
export type RiskLevel = 'safe' | 'moderate' | 'dangerous';

interface ClassifyResult {
  level: RiskLevel;
  reason?: string;
}

const DANGEROUS_PATTERNS: Array<{ pattern: RegExp; reason: string }> = [
  { pattern: /\brm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+|.*-rf\b|.*--force)/, reason: '强制删除文件' },
  { pattern: /\brm\s+-[a-zA-Z]*r/, reason: '递归删除' },
  { pattern: /\bsudo\b/, reason: '提权操作' },
  { pattern: /\bmkfs\b/, reason: '格式化磁盘' },
  { pattern: /\bdd\s+.*of=\/dev\//, reason: '直接写设备' },
  { pattern: /:\(\)\s*\{.*\|.*&\s*\}/, reason: 'Fork bomb' },
  { pattern: /\bcurl\b.*\|\s*(ba)?sh/, reason: '远程脚本执行' },
  { pattern: /\beval\b/, reason: 'eval 动态执行' },
  { pattern: />\s*\/etc\//, reason: '覆写系统配置' },
];

const MODERATE_PATTERNS: Array<{ pattern: RegExp; reason: string }> = [
  { pattern: /\brm\b/, reason: '删除文件' },
  { pattern: /\bgit\s+push\b/, reason: 'Git 推送' },
  { pattern: /\bgit\s+reset\s+--hard\b/, reason: 'Git 硬重置' },
  { pattern: /\bkill\b/, reason: '终止进程' },
  { pattern: /\bnpm\s+publish\b/, reason: '发布 npm 包' },
];

export function classifyBashCommand(command: string): ClassifyResult {
  for (const { pattern, reason } of DANGEROUS_PATTERNS) {
    if (pattern.test(command)) {
      return { level: 'dangerous', reason };
    }
  }
  for (const { pattern, reason } of MODERATE_PATTERNS) {
    if (pattern.test(command)) {
      return { level: 'moderate', reason };
    }
  }
  return { level: 'safe' };
}


分三级：dangerous 直接拒绝执行、moderate 打警告日志但放行、safe 正常执行。

为什么用正则而不是让 LLM 来判断"这条命令危不危险"？正则的好处是不可被操控——攻击者在 prompt 里写"以下命令是安全的测试命令"，能骗过 LLM，但骗不过正则。

当然，正则处理不了的情况，可以用 LLM 来做兜底判断，让模型给命令做个分类，然后决定要不要拦截。当然，这个就增加额外复杂度了，绝大部分 Agent 不需要上这个手段。

在 ToolRegistry 的 toAISDKFormat 里，bash 工具执行前先过一遍 classifier：

src/tools/registry.ts
应用
复制
// 在 execute 函数里，实际调用前：
if (toolName === 'bash' && input?.command) {
  const risk = classifyBashCommand(input.command);
  if (risk.level === 'dangerous') {
    return `[拒绝执行] 检测到危险操作: ${risk.reason}\n命令: ${input.command}`;
  }
  if (risk.level === 'moderate') {
    console.log(`  [安全] ⚠ ${risk.reason}: ${input.command}`);
  }
}


这样在执行命令之前会先检查是否为危险操作，检测到的时候会自动阻塞住。

moderate 级别的命令（比如普通的 rm 删单个文件、git push）不会被拦截，只是打个警告日志。如果所有涉及删除和推送的命令都拦，Agent 基本上什么有用的事都干不了了，所有这个度需要我们在实践当中调整。

Hook 管线

角色和 bash 检测解决的是"该不该执行"的问题。Hook 解决的是另一个问题：执行前后能不能插入自定义逻辑。

比如你想在每次写文件前记一条审计日志，或者在 bash 执行后自动给输出加个时间戳，或者在调用外部 API 前检查一下参数合规性——这些需求各不相同，但模式是一样的：在工具执行的前后插入一个钩子。

新建 src/security/hooks.ts：

src/security/hooks.ts
应用
复制
export type HookAction = 'allow' | 'block' | 'modify';

export interface HookResult {
  action: HookAction;
  reason?: string;
  modifiedInput?: unknown;
  modifiedOutput?: unknown;
}

export type PreToolHook = (toolName: string, input: unknown) => HookResult | Promise<HookResult>;
export type PostToolHook = (toolName: string, input: unknown, output: unknown) => HookResult | Promise<HookResult>;

export class HookPipeline {
  private preHooks: Array<{ name: string; fn: PreToolHook }> = [];
  private postHooks: Array<{ name: string; fn: PostToolHook }> = [];

  registerPre(name: string, fn: PreToolHook): void {
    this.preHooks.push({ name, fn });
  }

  registerPost(name: string, fn: PostToolHook): void {
    this.postHooks.push({ name, fn });
  }

  async runPre(toolName: string, input: unknown): Promise<HookResult> {
    let currentInput = input;
    for (const hook of this.preHooks) {
      try {
        const result = await hook.fn(toolName, currentInput);
        if (result.action === 'block') {
          console.log(`  [hook:${hook.name}] 拦截 ${toolName}: ${result.reason}`);
          return result;
        }
        if (result.action === 'modify' && result.modifiedInput !== undefined) {
          currentInput = result.modifiedInput;
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [hook:${hook.name}] pre 异常: ${msg}`);
      }
    }
    return { action: 'allow' };
  }

  async runPost(toolName: string, input: unknown, output: unknown): Promise<HookResult> {
    let currentOutput = output;
    for (const hook of this.postHooks) {
      try {
        const result = await hook.fn(toolName, input, currentOutput);
        if (result.action === 'modify' && result.modifiedOutput !== undefined) {
          currentOutput = result.modifiedOutput;
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [hook:${hook.name}] post 异常: ${msg}`);
      }
    }
    return { action: 'allow', modifiedOutput: currentOutput };
  }
}


Hook 的返回值是三选一：allow（放行）、block（拦截，只在 pre hook 有效）、modify（放行但修改输入/输出）。多个 hook 串联执行，前一个的修改结果会传给下一个——管线模式（Pipeline）。

有两个设计细节值得注意。

一是每个 hook 的异常被 catch 住了——跟 Plugin 的错误隔离一样，一个 hook 挂了不影响其他 hook 和工具本身的执行。二是 hook 的配置不能被 Agent 自己修改——hook 是在启动时注册的，Agent 运行过程中没有 API 来增删 hook。

这是出于安全的考量：如果 Agent 能修改自己的安全检查，那 prompt injection 就能让它把所有检查关掉。Claude Code 也是这个设计——hook 配置在 .claude/settings.json 里，Agent 不能写这个文件。

还有一个 list() 方法，返回所有已注册 hook 的名称——前面 createSecurityCommands 里的 /hooks 命令就是调它来展示的：

ts
复制
list(): { pre: string[]; post: string[] } {
  return {
    pre: this.preHooks.map(h => h.name),
    post: this.postHooks.map(h => h.name),
  };
}

Hook 怎么接入 ToolRegistry

HookPipeline 写好了，但它跟工具执行流程还没有关系。要让 hook 在每次工具调用时自动触发，需要把它注入到 ToolRegistry 里。ToolRegistry 新增一个 setHookPipeline() 方法，在 toAISDKFormat 生成的 execute 函数里，工具实际执行前后分别调用 hookPipeline.runPre() 和 hookPipeline.runPost()：

src/tools/registry.ts
应用
复制
// ToolRegistry 新增方法
setHookPipeline(pipeline: HookPipeline): void {
  this.hookPipeline = pipeline;
}

// toAISDKFormat 的 execute 里，工具执行前：
if (hookPipeline) {
  const preResult = await hookPipeline.runPre(toolName, input);
  if (preResult.action === 'block') {
    return `[Hook 拦截] ${preResult.reason || '操作被阻止'}`;
  }
  if (preResult.action === 'modify' && preResult.modifiedInput !== undefined) {
    input = preResult.modifiedInput;
  }
}

// 工具执行后：
if (hookPipeline) {
  const postResult = await hookPipeline.runPost(toolName, input, output);
  if (postResult.modifiedOutput !== undefined) {
    output = String(postResult.modifiedOutput);
  }
}


pre hook 在 bash classifier 之后、工具执行之前运行。如果返回 block，工具直接不执行；如果返回 modify，用修改后的 input 继续走。post hook 在工具返回结果之后运行，能修改输出内容。整个执行顺序是：角色过滤 → bash classifier → pre hook → 工具执行 → post hook。

注册两个示例 hook 感受一下：

src/index.ts
应用
复制
// 示例 Pre Hook: 写文件前记录审计日志
hookPipeline.registerPre('audit-log', (toolName, input) => {
  if (toolName === 'write_file' || toolName === 'edit_file') {
    const path = (input as any)?.path || 'unknown';
    console.log(`  [audit] 文件写入操作: ${toolName} → ${path}`);
  }
  return { action: 'allow' };
});

// 示例 Post Hook: 给 bash 输出加时间戳
hookPipeline.registerPost('bash-timestamp', (toolName, _input, output) => {
  if (toolName === 'bash') {
    const timestamp = new Date().toISOString();
    return {
      action: 'modify',
      modifiedOutput: `[${timestamp}]\n${output}`,
    };
  }
  return { action: 'allow' };
});

bash
运行
复制
pnpm start

text
复制
You: 测试bash

--- Step 1 ---
  [调用: bash({"command":"echo \"Hello from bash!\" && date"})]
  [hook:bash-timestamp] 修改了 bash 的输出
  [结果: bash] [2026-05-05T05:56:18.328Z]
Hello from bash!
Mon May  5 13:56:18 CST 2026


bash 输出自动加了 ISO 时间戳——post hook 生效了。

text
复制
You: 测试写文件

--- Step 1 ---
  [audit] 文件写入操作: write_file → test-output.txt
  [调用: write_file({"path":"test-output.txt","content":"Hello from hook test"})]
  [结果: write_file] 已写入 20 字符到 test-output.txt


写文件前 pre hook 打了一条审计日志。在生产环境里，这条日志可以写到文件、发到日志平台、或者存到数据库——谁在什么时候改了什么文件，一目了然。

你也可以写更复杂的 hook。比如一个"敏感词过滤"的 pre hook：检测到用户输入里有 API Key 之类的敏感内容时自动脱敏。或者一个"结果格式化"的 post hook：把 JSON 输出自动格式化成表格。

Hook 的注册方式跟 Plugin 注册工具一样简单——未来 Plugin 也能通过 api.registerHook 注册自己的 hook，安全策略也变成了可插拔的，非常灵活。如果你感兴趣，可以在我们的插件机制里面支持上 Hook 的扩展方式。

index.ts 里的完整串联

角色、bash classifier、hook 管线、安全命令——这些零件都造好了，在 index.ts 里把它们串起来：

src/index.ts
应用
复制
import { HookPipeline } from './security/hooks.js';
import { classifyBashCommand } from './security/bash-classifier.js';
import { createSecurityCommands } from './commands/security.js';

// ── Security: Hook Pipeline ────────────────────────────────
const hookPipeline = new HookPipeline();

// 注册示例 hook（audit-log、bash-timestamp，前面已经看过了）
// ...

// 把 hook 管线注入 registry
registry.setHookPipeline(hookPipeline);

// 在 dispatcher 里注册安全命令
const dispatch = createDispatcher([
  // ...其他命令组...
  ...createSecurityCommands(registry, hookPipeline),
]);


关键就三行：new HookPipeline() 创建管线实例，registry.setHookPipeline(hookPipeline) 让工具执行流程能调用 hook，createSecurityCommands(registry, hookPipeline) 注册 /role 和 /hooks 命令。

启动时我们还打印了当前安全状态，方便用户一眼看到角色和 hook 配置：

ts
复制
const role = registry.getRole();
const toolCount = registry.getActiveTools().length;
const hooks = hookPipeline.list();

console.log(`  当前角色: ${role}，可用工具: ${toolCount} 个`);
console.log(`  Hook: ${hooks.pre.length} 个 pre + ${hooks.post.length} 个 post`);


你也可以试试 /hooks 命令看一下当前注册了哪些 hook：

text
复制
You: /hooks

[hooks]
  Pre-Tool Hooks:
    - audit-log
  Post-Tool Hooks:
    - bash-timestamp

三层安全的协作关系

到这里我们一共做了三层安全措施，它们各自解决不同层面的问题：

角色权限是第一道门——在工具暴露给模型之前，先按角色过滤掉不该用的工具。guest 连 bash 这个工具的存在都看不到，自然也不会去调用。

Bash Classifier 是第二道门——即使 owner 有权限用 bash，rm -rf 这种命令也会被拦下来。这层防的是误操作和 prompt injection。

Hook 管线是第三层——不是为了拦截，而是为了可观测性和可扩展性。审计日志、格式检查、参数改写——这些不影响"能不能执行"，但影响"执行的质量和可追溯性"。生产环境里出了问题要回溯，hook 可以很好帮你完成这件事。

这三层互不依赖，各自独立工作。你可以只用角色权限不用 hook，也可以只用 hook 不做角色过滤——按你的场景自由组合。实际上，很多团队一开始只需要角色权限，等 Agent 上线跑了一段时间、发现需要审计和检查的时候，再加 hook 就行。

写在最后

我们这一节建立的安全体系是可组合的：角色管"谁能用什么"，bash classifier 管"什么命令不能跑"，hook 管"执行前后插什么逻辑"。这三个机制以后还能继续扩展——比如基于 Channel 来源自动分配角色（飞书群里的人默认 collaborator，终端里默认 owner），或者让 Plugin 注册自己的安全 hook。

下一节我们做 Cron 定时任务系统——让 Agent 不只是被动等你发消息，而是能自己主动做事：定时查日志、每天早上发简报、每小时跑一次数据检查。我们下一小节，再见。

参考资料
Claude Code 权限模型 — 6 种权限模式的设计
OWASP Top 10 for LLM Applications — LLM 安全风险清单
上一篇
Channel 抽象——让 Agent 活在飞书群里
下一篇 · 第六章：权限 + Cron + Multi-Agent
让 Agent 自己动起来——Cron 定时任务系统
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
export type Role = 'owner' | 'collaborator' | 'guest';

export interface UserIdentity {
  id: string;
  name: string;
  role: Role;
}

const TOOL_ACCESS: Record<Role, { allow: string[] | '*'; deny: string[] }> = {
  owner: {
    allow: '*',
    deny: [],
  },
  collaborator: {
    allow: '*',
    deny: ['bash'],
  },
  guest: {
    allow: ['get_weather', 'calculator', 'read_file', 'list_directory', 'glob', 'grep', 'rag_search'],
    deny: [],
  },
};

export function canUseTool(role: Role, toolName: string): boolean {
  const access = TOOL_ACCESS[role];
  if (access.deny.includes(toolName)) return false;
  if (access.allow === '*') return true;
  return access.allow.includes(toolName);
}

export function filterToolsForRole(toolNames: string[], role: Role): string[] {
  return toolNames.filter(name => canUseTool(role, name));
}
```


```ts
getActiveTools(): ToolDefinition[] {
  return this.getAll().filter(tool => {
    // ...原有的 profile 和 defer 过滤...
    if (!canUseTool(this.currentRole, tool.name)) {
      return false;  // ← 角色不允许的工具直接不暴露给模型
    }
    return true;
  });
}
```


```ts
import type { CommandHandler } from './index.js';
import type { ToolRegistry } from '../tools/registry.js';
import type { HookPipeline } from '../security/hooks.js';
import type { Role } from '../security/roles.js';

export function createSecurityCommands(
  registry: ToolRegistry,
  hookPipeline: HookPipeline,
): CommandHandler[] {
  return [
    // /role [owner|collaborator|guest]
    (cmd, _ctx) => {
      const match = cmd.match(/^\/role(?:\s+(owner|collaborator|guest))?$/);
      if (!match) return false;

      if (match[1]) {
        const role = match[1] as Role;
        registry.setRole(role);
        const toolCount = registry.getActiveTools().length;
        console.log(`\n[security] 角色切换为 ${role}，可用工具: ${toolCount} 个\n`);
      } else {
        const role = registry.getRole();
        const toolCount = registry.getActiveTools().length;
        console.log(`\n[security] 当前角色: ${role}，可用工具: ${toolCount} 个\n`);
      }
      return true;
    },

    // /hooks
    (cmd, _ctx) => {
      if (cmd !== '/hooks') return false;

      const hooks = hookPipeline.list();
      console.log('\n[hooks]');
      if (hooks.pre.length > 0) {
        console.log('  Pre-Tool Hooks:');
        for (const name of hooks.pre) console.log(`    - ${name}`);
      }
      if (hooks.post.length > 0) {
        console.log('  Post-Tool Hooks:');
        for (const name of hooks.post) console.log(`    - ${name}`);
      }
      if (hooks.pre.length === 0 && hooks.post.length === 0) {
        console.log('  没有注册的 Hook');
      }
      console.log('');
      return true;
    },
  ];
}
```


```bash
pnpm start
```


```text
You: /role
[security] 当前角色: owner，可用工具: 16 个

You: /role guest
[security] 角色切换为 guest，可用工具: 7 个

You: /role owner
[security] 角色切换为 owner，可用工具: 16 个
```


```ts
export type RiskLevel = 'safe' | 'moderate' | 'dangerous';

interface ClassifyResult {
  level: RiskLevel;
  reason?: string;
}

const DANGEROUS_PATTERNS: Array<{ pattern: RegExp; reason: string }> = [
  { pattern: /\brm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+|.*-rf\b|.*--force)/, reason: '强制删除文件' },
  { pattern: /\brm\s+-[a-zA-Z]*r/, reason: '递归删除' },
  { pattern: /\bsudo\b/, reason: '提权操作' },
  { pattern: /\bmkfs\b/, reason: '格式化磁盘' },
  { pattern: /\bdd\s+.*of=\/dev\//, reason: '直接写设备' },
  { pattern: /:\(\)\s*\{.*\|.*&\s*\}/, reason: 'Fork bomb' },
  { pattern: /\bcurl\b.*\|\s*(ba)?sh/, reason: '远程脚本执行' },
  { pattern: /\beval\b/, reason: 'eval 动态执行' },
  { pattern: />\s*\/etc\//, reason: '覆写系统配置' },
];

const MODERATE_PATTERNS: Array<{ pattern: RegExp; reason: string }> = [
  { pattern: /\brm\b/, reason: '删除文件' },
  { pattern: /\bgit\s+push\b/, reason: 'Git 推送' },
  { pattern: /\bgit\s+reset\s+--hard\b/, reason: 'Git 硬重置' },
  { pattern: /\bkill\b/, reason: '终止进程' },
  { pattern: /\bnpm\s+publish\b/, reason: '发布 npm 包' },
];

export function classifyBashCommand(command: string): ClassifyResult {
  for (const { pattern, reason } of DANGEROUS_PATTERNS) {
    if (pattern.test(command)) {
      return { level: 'dangerous', reason };
    }
  }
  for (const { pattern, reason } of MODERATE_PATTERNS) {
    if (pattern.test(command)) {
      return { level: 'moderate', reason };
    }
  }
  return { level: 'safe' };
}
```


```ts
// 在 execute 函数里，实际调用前：
if (toolName === 'bash' && input?.command) {
  const risk = classifyBashCommand(input.command);
  if (risk.level === 'dangerous') {
    return `[拒绝执行] 检测到危险操作: ${risk.reason}\n命令: ${input.command}`;
  }
  if (risk.level === 'moderate') {
    console.log(`  [安全] ⚠ ${risk.reason}: ${input.command}`);
  }
}
```


```ts
export type HookAction = 'allow' | 'block' | 'modify';

export interface HookResult {
  action: HookAction;
  reason?: string;
  modifiedInput?: unknown;
  modifiedOutput?: unknown;
}

export type PreToolHook = (toolName: string, input: unknown) => HookResult | Promise<HookResult>;
export type PostToolHook = (toolName: string, input: unknown, output: unknown) => HookResult | Promise<HookResult>;

export class HookPipeline {
  private preHooks: Array<{ name: string; fn: PreToolHook }> = [];
  private postHooks: Array<{ name: string; fn: PostToolHook }> = [];

  registerPre(name: string, fn: PreToolHook): void {
    this.preHooks.push({ name, fn });
  }

  registerPost(name: string, fn: PostToolHook): void {
    this.postHooks.push({ name, fn });
  }

  async runPre(toolName: string, input: unknown): Promise<HookResult> {
    let currentInput = input;
    for (const hook of this.preHooks) {
      try {
        const result = await hook.fn(toolName, currentInput);
        if (result.action === 'block') {
          console.log(`  [hook:${hook.name}] 拦截 ${toolName}: ${result.reason}`);
          return result;
        }
        if (result.action === 'modify' && result.modifiedInput !== undefined) {
          currentInput = result.modifiedInput;
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [hook:${hook.name}] pre 异常: ${msg}`);
      }
    }
    return { action: 'allow' };
  }

  async runPost(toolName: string, input: unknown, output: unknown): Promise<HookResult> {
    let currentOutput = output;
    for (const hook of this.postHooks) {
      try {
        const result = await hook.fn(toolName, input, currentOutput);
        if (result.action === 'modify' && result.modifiedOutput !== undefined) {
          currentOutput = result.modifiedOutput;
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [hook:${hook.name}] post 异常: ${msg}`);
      }
    }
    return { action: 'allow', modifiedOutput: currentOutput };
  }
}
```


```ts
list(): { pre: string[]; post: string[] } {
  return {
    pre: this.preHooks.map(h => h.name),
    post: this.postHooks.map(h => h.name),
  };
}
```


```ts
// ToolRegistry 新增方法
setHookPipeline(pipeline: HookPipeline): void {
  this.hookPipeline = pipeline;
}

// toAISDKFormat 的 execute 里，工具执行前：
if (hookPipeline) {
  const preResult = await hookPipeline.runPre(toolName, input);
  if (preResult.action === 'block') {
    return `[Hook 拦截] ${preResult.reason || '操作被阻止'}`;
  }
  if (preResult.action === 'modify' && preResult.modifiedInput !== undefined) {
    input = preResult.modifiedInput;
  }
}

// 工具执行后：
if (hookPipeline) {
  const postResult = await hookPipeline.runPost(toolName, input, output);
  if (postResult.modifiedOutput !== undefined) {
    output = String(postResult.modifiedOutput);
  }
}
```


```ts
// 示例 Pre Hook: 写文件前记录审计日志
hookPipeline.registerPre('audit-log', (toolName, input) => {
  if (toolName === 'write_file' || toolName === 'edit_file') {
    const path = (input as any)?.path || 'unknown';
    console.log(`  [audit] 文件写入操作: ${toolName} → ${path}`);
  }
  return { action: 'allow' };
});

// 示例 Post Hook: 给 bash 输出加时间戳
hookPipeline.registerPost('bash-timestamp', (toolName, _input, output) => {
  if (toolName === 'bash') {
    const timestamp = new Date().toISOString();
    return {
      action: 'modify',
      modifiedOutput: `[${timestamp}]\n${output}`,
    };
  }
  return { action: 'allow' };
});
```


```bash
pnpm start
```


```text
You: 测试bash

--- Step 1 ---
  [调用: bash({"command":"echo \"Hello from bash!\" && date"})]
  [hook:bash-timestamp] 修改了 bash 的输出
  [结果: bash] [2026-05-05T05:56:18.328Z]
Hello from bash!
Mon May  5 13:56:18 CST 2026
```


```text
You: 测试写文件

--- Step 1 ---
  [audit] 文件写入操作: write_file → test-output.txt
  [调用: write_file({"path":"test-output.txt","content":"Hello from hook test"})]
  [结果: write_file] 已写入 20 字符到 test-output.txt
```


```ts
import { HookPipeline } from './security/hooks.js';
import { classifyBashCommand } from './security/bash-classifier.js';
import { createSecurityCommands } from './commands/security.js';

// ── Security: Hook Pipeline ────────────────────────────────
const hookPipeline = new HookPipeline();

// 注册示例 hook（audit-log、bash-timestamp，前面已经看过了）
// ...

// 把 hook 管线注入 registry
registry.setHookPipeline(hookPipeline);

// 在 dispatcher 里注册安全命令
const dispatch = createDispatcher([
  // ...其他命令组...
  ...createSecurityCommands(registry, hookPipeline),
]);
```


```ts
const role = registry.getRole();
const toolCount = registry.getActiveTools().length;
const hooks = hookPipeline.list();

console.log(`  当前角色: ${role}，可用工具: ${toolCount} 个`);
console.log(`  Hook: ${hooks.pre.length} 个 pre + ${hooks.post.length} 个 post`);
```


```text
You: /hooks

[hooks]
  Pre-Tool Hooks:
    - audit-log
  Post-Tool Hooks:
    - bash-timestamp
```
