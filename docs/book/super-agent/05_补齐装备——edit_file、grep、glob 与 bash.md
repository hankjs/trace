# 补齐装备——edit_file、grep、glob 与 bash

课程
Super Agent 实战课
补齐装备——edit_file、grep、glob 与 bash
补齐装备——edit_file、grep、glob 与 bash

约 18 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇搭了 ToolRegistry 和执行管线，但 Agent 手里的工具还是太少了——能读文件、能列目录，但不能编辑文件、不能搜索内容、不能跑命令。你让它"帮我把 package.json 里的版本号改成 2.0"，它只能 write_file 把整个文件重写一遍。

这篇我们来给现在的工具系统加入更多的"装备"。ToolRegistry 和并发控制的架构不用动，就是往里面注册四个新工具。每个工具的实现都有值得聊的细节。

先装依赖：

bash
运行
复制
pnpm install

edit_file：精确替换，不是全量覆写

write_file 的问题是它会覆盖整个文件。模型要改一行代码，得把整个文件内容重新生成一遍——不仅浪费 token，还容易出错（模型可能"不小心"改了其他地方）。

edit_file 的思路完全不同：你告诉我要替换哪段文本（old_string），替换成什么（new_string），我只改你指定的部分。值得一提的是，这就是 Claude Code 的 Edit 工具采用的方式。

src/tools.ts
应用
复制
export const editFileTool: ToolDefinition = {
  name: 'edit_file',
  description: '精确替换文件中的指定内容。用 old_string 定位要替换的文本，用 new_string 替换它。不是全量覆写——只改你指定的部分',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '文件路径' },
      old_string: { type: 'string', description: '要被替换的原始文本（必须精确匹配）' },
      new_string: { type: 'string', description: '替换后的新文本' },
    },
    required: ['path', 'old_string', 'new_string'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  execute: async ({ path, old_string, new_string }) => {
    const resolved = resolve(path);
    if (!existsSync(resolved)) return `文件不存在: ${path}`;

    const content = readFileSync(resolved, 'utf-8');
    const count = content.split(old_string).length - 1;

    if (count === 0) {
      return `未找到匹配内容。请检查 old_string 是否与文件中的文本完全一致（包括空格和换行）`;
    }
    if (count > 1) {
      return `找到 ${count} 处匹配，请提供更多上下文让 old_string 唯一`;
    }

    const updated = content.replace(old_string, new_string);
    writeFileSync(resolved, updated, 'utf-8');
    return `已替换 ${path} 中的内容（${old_string.length} → ${new_string.length} 字符）`;
  },
};


这里有两个关键的错误处理：

匹配数为 0：模型给的 old_string 在文件里根本不存在。最常见的原因是模型记错了缩进或者多了少了一个空格。返回明确的错误信息，让模型知道"你的匹配不对，再看看"。

匹配数大于 1：old_string 在文件里出现了多次。这时候直接替换会改错地方。让模型提供更多上下文（比如多包含前后几行），确保唯一匹配。

这两种错误信息的设计很重要——它们不是给人看的，是给模型看的。模型收到错误后会根据信息调整策略：匹配不到就换个写法，多个匹配就扩大范围。好的错误信息能让模型自我修正，差的错误信息只会让模型更困惑。

跑起来试试：

bash
运行
复制
pnpm start

You: 测试编辑

--- Step 1 ---
  [串行] edit_file 获取独占锁，等待其他工具完成
  [调用: edit_file({"path":"sample-data.txt","old_string":"一、工具注册机制","new_string":"一、工具注册机制（已更新）"})]
  [结果: edit_file] 已替换 sample-data.txt 中的内容（8 → 13 字符）


注意 [串行] edit_file 获取独占锁——edit_file 是写操作（isConcurrencySafe: false），上一篇实现的读写锁自动生效了。

glob：按模式找文件

Agent 要修改代码，第一步往往是"先看看项目里有哪些文件"。list_directory 只能看一层目录，glob 可以递归搜索。

src/tools.ts
应用
复制
export const globTool: ToolDefinition = {
  name: 'glob',
  description: '按模式搜索文件。支持 * 和 ** 通配符，如 "src/**/*.ts" 匹配 src 下所有 TypeScript 文件',
  parameters: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: '搜索模式，如 "**/*.ts"、"src/*.json"' },
      path: { type: 'string', description: '搜索起始目录，默认当前目录' },
    },
    required: ['pattern'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ pattern, path = '.' }) => {
    // ... 递归遍历目录，匹配模式 ...
    // 自动跳过 node_modules 和 .git
    // 结果上限 100 条，防止大项目撑爆
  },
};


应用代码后，完整实现会自动配置好。实现里有几个细节：自动跳过 node_modules 和 .git（这两个目录一搜就是几万条结果），结果上限 100 条防止大项目撑爆上下文。

bash
运行
复制
pnpm start

You: 测试glob

--- Step 1 ---
  [并发] glob 获取共享锁
  [调用: glob({"pattern":"**/*.ts"})]
  [结果: glob] src/agent-loop.ts
src/index.ts
src/loop-detection.ts
src/mock-model.ts
src/retry.ts
src/tool-registry.ts
src/tools.ts


glob 是只读操作，拿的是共享锁。

grep：搜内容比找文件更重要

模型要改一个函数，第一步不是读整个文件，而是 grep 找到函数在哪里、被谁调用了。

src/tools.ts
应用
复制
export const grepTool: ToolDefinition = {
  name: 'grep',
  description: '在文件中搜索匹配指定模式的内容。返回匹配的行号和内容',
  parameters: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: '搜索模式（正则表达式）' },
      path: { type: 'string', description: '搜索路径（文件或目录），默认当前目录' },
    },
    required: ['pattern'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ pattern, path = '.' }) => {
    // ... 递归搜索文件，正则匹配每一行 ...
    // 跳过 node_modules、.git、二进制文件
    // 返回格式：文件名:行号: 匹配内容
    // 上限 50 条匹配
  },
};


返回格式是 文件名:行号: 内容——跟终端里用 grep -rn 的输出一样。模型对这个格式非常熟悉，能直接从结果里提取文件路径和行号，然后调 read_file 或 edit_file 精准操作。

bash
运行
复制
pnpm start

You: 测试搜索

--- Step 1 ---
  [并发] grep 获取共享锁
  [调用: grep({"pattern":"export","path":"src"})]
  [结果: grep] agent-loop.ts:10: export async function agentLoop(
loop-detection.ts:5: export interface ToolCallRecord {
...


grep 也设了 maxResultChars: 3000，搜到太多结果时会被 ToolRegistry 的截断逻辑自动处理。

bash：最强也最危险的工具

bash 是万能工具——能跑测试、能装依赖、能查环境、能做任何 shell 能做的事。但也正因为如此，它是最危险的。一个 rm -rf / 就能把系统搞挂。

src/tools.ts
应用
复制
export const bashTool: ToolDefinition = {
  name: 'bash',
  description: '执行 shell 命令并返回输出。适合运行脚本、检查环境、执行构建等操作',
  parameters: {
    type: 'object',
    properties: {
      command: { type: 'string', description: '要执行的 shell 命令' },
    },
    required: ['command'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  maxResultChars: 3000,
  execute: async ({ command }) => {
    // 先检测环境是否支持 child_process
    try {
      execSync('echo test', { stdio: 'ignore' });
    } catch {
      return `[bash 不可用] 当前环境不支持 shell 命令。本地终端运行可使用。`;
    }

    try {
      const output = execSync(command, {
        encoding: 'utf-8',
        timeout: 10000,  // 10 秒超时
        maxBuffer: 1024 * 1024,
      });
      return output || '(命令执行成功，无输出)';
    } catch (err: any) {
      return `命令执行失败 (exit ${err.status || 1}):\n${err.stderr || err.message}`;
    }
  },
};


几个设计决策在这里说明一下：

超时 10 秒：防止模型跑一个 while true 卡死整个进程。生产环境通常更长（30-60 秒），但演示用 10 秒够了，你可以自行调整。

环境检测：WebContainer（浏览器沙箱）里 child_process 不可用，所以 execute 一开始先检测环境。不可用就直接返回提示信息，不 crash。

isConcurrencySafe: false：bash 命令可能有副作用（创建文件、修改环境变量），所以默认串行。生产级 Agent 会更精细——分析命令内容判断是否只读（ls、cat 可以并发，rm、mv 必须串行），但那个逻辑比较复杂，后面权限系统那篇再加。

bash
运行
复制
pnpm start

You: 测试bash

--- Step 1 ---
  [串行] bash 获取独占锁，等待其他工具完成
  [调用: bash({"command":"echo \"Hello from bash!\" && date"})]
  [结果: bash] Hello from bash!
Sat Apr 19 2026 19:00:00 GMT+0800 (CST)


在 WebContainer 环境里，bash 工具会提示不可用。本地终端 pnpm start 可以正常使用。

到这里，Agent 的工具箱从 5 个扩充到了 9 个。来看看完整阵容：

工具	用途	并发	只读
get_weather	查天气（演示用）	可并发	只读
calculator	数学计算	可并发	只读
read_file	读文件	可并发	只读
write_file	写文件	串行	读写
list_directory	列目录	可并发	只读
edit_file	精确编辑	串行	读写
glob	文件搜索	可并发	只读
grep	内容搜索	可并发	只读
bash	命令执行	串行	读写

有了这些工具，Agent 已经能做不少正经事了——找文件、搜代码、精确编辑、跑命令。下一篇我们接 MCP 协议，让 Agent 还能调用你自己没写过的外部工具。

参考链接
Claude Code - Tool Overview
Anthropic - Tool Use Best Practices
OpenAI - Function Calling
上一篇
给 Agent 一双手——Tool 注册、执行、截断与并发
下一篇 · 第二章：Tool System
小试牛刀——把工具组装成应用：代码分析、Research Agent、Vibe Coding
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
export const editFileTool: ToolDefinition = {
  name: 'edit_file',
  description: '精确替换文件中的指定内容。用 old_string 定位要替换的文本，用 new_string 替换它。不是全量覆写——只改你指定的部分',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '文件路径' },
      old_string: { type: 'string', description: '要被替换的原始文本（必须精确匹配）' },
      new_string: { type: 'string', description: '替换后的新文本' },
    },
    required: ['path', 'old_string', 'new_string'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  execute: async ({ path, old_string, new_string }) => {
    const resolved = resolve(path);
    if (!existsSync(resolved)) return `文件不存在: ${path}`;

    const content = readFileSync(resolved, 'utf-8');
    const count = content.split(old_string).length - 1;

    if (count === 0) {
      return `未找到匹配内容。请检查 old_string 是否与文件中的文本完全一致（包括空格和换行）`;
    }
    if (count > 1) {
      return `找到 ${count} 处匹配，请提供更多上下文让 old_string 唯一`;
    }

    const updated = content.replace(old_string, new_string);
    writeFileSync(resolved, updated, 'utf-8');
    return `已替换 ${path} 中的内容（${old_string.length} → ${new_string.length} 字符）`;
  },
};
```


```bash
pnpm start
```


```
You: 测试编辑

--- Step 1 ---
  [串行] edit_file 获取独占锁，等待其他工具完成
  [调用: edit_file({"path":"sample-data.txt","old_string":"一、工具注册机制","new_string":"一、工具注册机制（已更新）"})]
  [结果: edit_file] 已替换 sample-data.txt 中的内容（8 → 13 字符）
```


```typescript
export const globTool: ToolDefinition = {
  name: 'glob',
  description: '按模式搜索文件。支持 * 和 ** 通配符，如 "src/**/*.ts" 匹配 src 下所有 TypeScript 文件',
  parameters: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: '搜索模式，如 "**/*.ts"、"src/*.json"' },
      path: { type: 'string', description: '搜索起始目录，默认当前目录' },
    },
    required: ['pattern'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ pattern, path = '.' }) => {
    // ... 递归遍历目录，匹配模式 ...
    // 自动跳过 node_modules 和 .git
    // 结果上限 100 条，防止大项目撑爆
  },
};
```


```bash
pnpm start
```


```
You: 测试glob

--- Step 1 ---
  [并发] glob 获取共享锁
  [调用: glob({"pattern":"**/*.ts"})]
  [结果: glob] src/agent-loop.ts
src/index.ts
src/loop-detection.ts
src/mock-model.ts
src/retry.ts
src/tool-registry.ts
src/tools.ts
```


```typescript
export const grepTool: ToolDefinition = {
  name: 'grep',
  description: '在文件中搜索匹配指定模式的内容。返回匹配的行号和内容',
  parameters: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: '搜索模式（正则表达式）' },
      path: { type: 'string', description: '搜索路径（文件或目录），默认当前目录' },
    },
    required: ['pattern'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ pattern, path = '.' }) => {
    // ... 递归搜索文件，正则匹配每一行 ...
    // 跳过 node_modules、.git、二进制文件
    // 返回格式：文件名:行号: 匹配内容
    // 上限 50 条匹配
  },
};
```


```bash
pnpm start
```


```
You: 测试搜索

--- Step 1 ---
  [并发] grep 获取共享锁
  [调用: grep({"pattern":"export","path":"src"})]
  [结果: grep] agent-loop.ts:10: export async function agentLoop(
loop-detection.ts:5: export interface ToolCallRecord {
...
```


```typescript
export const bashTool: ToolDefinition = {
  name: 'bash',
  description: '执行 shell 命令并返回输出。适合运行脚本、检查环境、执行构建等操作',
  parameters: {
    type: 'object',
    properties: {
      command: { type: 'string', description: '要执行的 shell 命令' },
    },
    required: ['command'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  maxResultChars: 3000,
  execute: async ({ command }) => {
    // 先检测环境是否支持 child_process
    try {
      execSync('echo test', { stdio: 'ignore' });
    } catch {
      return `[bash 不可用] 当前环境不支持 shell 命令。本地终端运行可使用。`;
    }

    try {
      const output = execSync(command, {
        encoding: 'utf-8',
        timeout: 10000,  // 10 秒超时
        maxBuffer: 1024 * 1024,
      });
      return output || '(命令执行成功，无输出)';
    } catch (err: any) {
      return `命令执行失败 (exit ${err.status || 1}):\n${err.stderr || err.message}`;
    }
  },
};
```


```bash
pnpm start
```


```
You: 测试bash

--- Step 1 ---
  [串行] bash 获取独占锁，等待其他工具完成
  [调用: bash({"command":"echo \"Hello from bash!\" && date"})]
  [结果: bash] Hello from bash!
Sat Apr 19 2026 19:00:00 GMT+0800 (CST)
```
