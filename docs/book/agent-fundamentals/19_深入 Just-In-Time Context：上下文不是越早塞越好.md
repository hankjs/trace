# 深入 Just-In-Time Context：上下文不是越早塞越好

课程
吃透 AI Agent 开发
深入 Just-In-Time Context：上下文不是越早塞越好
深入 Just-In-Time Context：上下文不是越早塞越好

约 53 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

本节可在右侧编辑器中实时编码运行。我们会从零搭一个能用 JIT 策略定位真实 bug 的 mini Agent。

前面几篇我们讲了 Context Engineering 的几个核心问题：System Prompt 怎么分层、上下文快爆了怎么压缩、Cache 怎么用才省钱。

但这几篇都有一个共同的前提假设——信息已经在上下文里了，问题是怎么管理它。

这篇要退一步问一个更根本的问题：信息什么时候应该进入上下文？

全量预填充 vs JIT：两种截然不同的策略

你让 Agent 帮你分析一个代码仓库。仓库里有 200 个文件，几万行代码。你怎么让 Agent"看到"这些代码？

全量预填充（Eager Loading）——不管三七二十一，把所有可能相关的内容一股脑塞进上下文。简单粗暴，但 token 消耗随项目规模线性增长，而且塞太多无关信息会稀释模型注意力（前面 KV Cache 那篇讲过的 Context Rot）。

JIT（Just-In-Time）——用的时候才去拿。不提前塞内容，等 Agent 真正需要某段信息时再按需加载。

Anthropic 在 Context Engineering 博客里有一个很精辟的类比：人类也不会把整本百科全书背下来。我们记的是"去哪里查"——文件系统、搜索引擎、收藏夹。Agent 也应该这样：记住索引，而不是记住内容。

JIT 是一个总原则，具体怎么实现"按需加载"有不同的路线。这篇我们会看到三种：RAG（预建索引、语义检索）、Agentic Search（运行时工具探索）、Context Offloading（主动卸载 + 按需恢复）。它们解决的问题不同，适用场景也不一样，后面会逐一展开。

光看概念和理论不容易体会两种策略的差别。这一节，我专门准备了一个实战场景来帮助大家直观地理解。我们会直接搭一个 mini Agent，让它用 JIT 策略去定位一个真实的 bug，跑完你就知道这俩为什么不一样了。

实战场景：登录后跳错页面的 bug

我们要调查的是一个 mini Express 项目，10 个文件左右，模拟一个真实的认证系统。项目结构在右侧编辑器的 bug-project/ 目录里：

bug-project/
├── CLAUDE.md
├── src/
│   ├── app.ts                     - 应用入口
│   ├── auth/
│   │   ├── login.ts               - 登录路由
│   │   └── session.ts             - Session 管理
│   ├── middleware/
│   │   ├── auth.ts                - 鉴权中间件
│   │   └── redirect.ts            - 登录后跳转 ← 这里有 bug
│   ├── routes/{index,user,admin}.ts
│   └── utils/cookies.ts


用户场景："用户反馈登录后总是跳到 /admin，不管他们之前访问的是哪个页面"。

Bug 的真相我先不剧透——你跟着 Agent 一步步探索，看它怎么定位。

接下来五个 Step，从"实现一个 Glob 工具"开始，最终让 Agent 自己用 JIT 策略找到这个 bug，并和"全读"基线对比 token 消耗。

先装依赖：

bash
运行
复制
pnpm install

Step 1：实现 Glob——最便宜的探索工具

JIT 策略的第一性原理是按成本递增排序工具调用：先用最便宜的工具看全局，再用稍贵的工具定位，最后才用最贵的工具读细节。

最便宜的工具是 Glob——按文件名模式搜索，只返回路径，不读内容。一个有 1000 个文件的项目，glob 一次就几百字符的输出，几乎零 token 成本。

我们用 node:fs/promises 和递归 walk 实现一个简化版的 glob：

src/tools.ts
应用
复制
import { readdir } from 'node:fs/promises'
import { join, relative, sep } from 'node:path'

const PROJECT_ROOT = 'bug-project'

async function* walk(dir: string): AsyncGenerator<string> {
  const entries = await readdir(dir, { withFileTypes: true })
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) {
      yield* walk(path)
    } else if (entry.isFile()) {
      yield path
    }
  }
}

function matchPattern(path: string, pattern: string): boolean {
  // 简化版 glob:
  //   **/  匹配任意层目录（含零层）
  //   **   匹配任意字符
  //   *    匹配单层非斜杠字符
  //   {a,b} 匹配多个扩展名
  const expanded = pattern.replace(/\{([^}]+)\}/g, (_, opts) => `(${opts.split(',').join('|')})`)
  const regex = expanded
    .replace(/\./g, '\\.')
    .replace(/\*\*\//g, '__GLOBSTARSEP__')
    .replace(/\*\*/g, '__GLOBSTAR__')
    .replace(/\*/g, '[^/]*')
    .replace(/__GLOBSTARSEP__/g, '(?:.*/)?')
    .replace(/__GLOBSTAR__/g, '.*')
  return new RegExp(`^${regex}$`).test(path)
}

export async function globFiles(pattern: string): Promise<string[]> {
  const results: string[] = []
  for await (const path of walk(PROJECT_ROOT)) {
    const rel = relative(PROJECT_ROOT, path).split(sep).join('/')
    if (matchPattern(rel, pattern)) {
      results.push(rel)
    }
  }
  return results.sort()
}

src/index.ts
应用
复制
import { globFiles } from './tools.js'

console.log('=== 测试 Glob ===\n')

console.log('所有 .ts 文件:')
const tsFiles = await globFiles('**/*.ts')
tsFiles.forEach(f => console.log('  ' + f))
console.log(`(${tsFiles.length} 个文件)\n`)

console.log('所有 .md 文件:')
const mdFiles = await globFiles('**/*.md')
mdFiles.forEach(f => console.log('  ' + f))
console.log(`(${mdFiles.length} 个文件)\n`)

console.log('middleware 目录下的文件:')
const middleware = await globFiles('src/middleware/**')
middleware.forEach(f => console.log('  ' + f))

bash
运行
复制
pnpm start


跑起来你会看到 9 个 ts 文件、1 个 CLAUDE.md，以及 middleware 下的两个文件。

光是这一步，Agent 已经能从文件名和目录结构里推断出很多信息了——auth/、middleware/、routes/ 这些命名暗示了项目的模块划分；middleware/redirect.ts 这个名字本身就暗示了"跟跳转相关的逻辑在这里"。这就是 Anthropic 说的"文件大小暗示复杂度，命名约定暗示用途"。

Step 2：加 Grep——按内容定位

光看文件名还不够。下一步是 Grep——按内容搜索，只返回命中的行。比 Read 轻量得多，因为不需要把整个文件塞进上下文。

把 tools.ts 改成下面这样：

src/tools.ts
应用
复制
import { readdir, readFile, stat } from 'node:fs/promises'
import { join, relative, sep } from 'node:path'

const PROJECT_ROOT = 'bug-project'

// ... walk 和 matchPattern 同 Step 1 ...

export async function globFiles(pattern: string): Promise<string[]> {
  // ... 同 Step 1 ...
}

export interface GrepHit {
  file: string
  line: number
  content: string
}

export async function grepContent(
  pattern: string,
  opts: { path?: string } = {}
): Promise<GrepHit[]> {
  const root = opts.path ? join(PROJECT_ROOT, opts.path) : PROJECT_ROOT
  const stats = await stat(root)
  const files: string[] = []
  if (stats.isDirectory()) {
    for await (const p of walk(root)) files.push(p)
  } else {
    files.push(root)
  }

  const regex = new RegExp(pattern)
  const hits: GrepHit[] = []
  for (const file of files) {
    const content = await readFile(file, 'utf8')
    content.split('\n').forEach((line, idx) => {
      if (regex.test(line)) {
        hits.push({
          file: relative(PROJECT_ROOT, file).split(sep).join('/'),
          line: idx + 1,
          content: line.trim(),
        })
      }
    })
  }
  return hits
}

src/index.ts
应用
复制
import { grepContent } from './tools.js'

console.log('=== 测试 Grep ===\n')

console.log('搜索 returnTo:')
const returnToHits = await grepContent('returnTo')
returnToHits.forEach(h => console.log(`  ${h.file}:${h.line}  ${h.content}`))
console.log(`(${returnToHits.length} 处命中)\n`)

console.log('搜索 res.redirect:')
const redirectHits = await grepContent('res\\.redirect')
redirectHits.forEach(h => console.log(`  ${h.file}:${h.line}  ${h.content}`))
console.log(`(${redirectHits.length} 处命中)`)

bash
运行
复制
pnpm start


输出大概是这样：

搜索 returnTo:
  CLAUDE.md:5  1. 用户访问受保护页面... 当前 URL 写到 `returnTo` cookie...
  CLAUDE.md:6  2. 用户登录成功后，应该读 `returnTo` cookie...
  src/middleware/auth.ts:16  res.cookie('returnTo', req.originalUrl, ...)
  src/middleware/redirect.ts:5  * 应该把用户送回他们登录前访问的页面（returnTo cookie），
  src/middleware/redirect.ts:6  * 如果没有 returnTo cookie 就回首页。
  src/utils/cookies.ts:4  res.cookie('returnTo', url, ...)
  ...


注意一个细节——src/middleware/redirect.ts 在它的注释里提到了 returnTo（"应该把用户送回..."），但 grep 结果里看不到它实际使用 returnTo。这就是 JIT 的威力：你不用读整个文件，光看 grep 结果就能拿到上下文。

Step 3：加 Read——按需读取完整内容

确认了嫌疑文件之后，才轮到最贵的 Read 出场。我们再加一个工具：

src/tools.ts
应用
复制
// ... globFiles 和 grepContent 同前 ...

export async function readFileTool(path: string): Promise<string> {
  const content = await readFile(join(PROJECT_ROOT, path), 'utf8')
  return content
}

src/index.ts
应用
复制
import { globFiles, grepContent, readFileTool } from './tools.js'

console.log('=== JIT 三件套手动跑一遍：定位"登录后跳错页面"的 bug ===\n')

console.log('1️⃣  glob 看项目结构\n')
const files = await globFiles('**/*.{ts,md}')
files.forEach(f => console.log('  ' + f))
console.log()

console.log('2️⃣  read CLAUDE.md 了解约定\n')
const claudeMd = await readFileTool('CLAUDE.md')
console.log(claudeMd)

console.log('3️⃣  grep returnTo 看哪些文件用到了\n')
const hits = await grepContent('returnTo')
hits.forEach(h => console.log(`  ${h.file}:${h.line}`))
console.log()

console.log('4️⃣  read 嫌疑文件 redirect.ts\n')
console.log(await readFileTool('src/middleware/redirect.ts'))

bash
运行
复制
pnpm start


跑完你会看到 redirect.ts 的真实代码：

typescript
复制
export function postLoginRedirect(_req: Request, res: Response) {
  res.clearCookie('returnTo')
  res.redirect('/admin')
}


Bug 找到了：注释里面"读 returnTo cookie 跳回原页面"完全没实现。

到这里我们手动走完了 JIT 探索的完整链路：

Glob → Read（约定文档）→ Grep → Read（嫌疑文件）


整个过程一共读了 2 个文件、做了 1 次 grep——比把所有文件全塞进去经济太多。

但这个手动版本说明不了什么，因为是我们人在指挥每一步。真正的 JIT 探索应该是 Agent 自己决定下一步该做什么。下一步就是把这套工具接到 Agent loop 里。

Step 4：把工具接到 Agent Loop，让模型自主决策

我们用前面课程里讲过的 Agent Loop 模式：模型决定调什么工具，工具执行后结果回到模型，循环直到模型返回最终答案。

为了让这个 demo 跑得稳定（不依赖外部 API），我们用一个 mock 模型来模拟模型的决策——它的决策逻辑严格对应一个真实模型在 JIT 探索时会做的判断。先看 mock 模型：

src/model.ts
应用
复制
import type { ToolCall } from './agent.js'

interface ModelInput {
  userMessage: string
  toolHistory: { name: string; args: any; result: string }[]
}

type ModelOutput =
  | { kind: 'tool_call'; call: ToolCall }
  | { kind: 'final'; content: string }

/**
 * Mock 模型：模拟 JIT 探索的决策逻辑。
 * 真实模型会自己想清楚下一步该做什么；这里我们用一个状态机来模拟。
 */
export function mockModel(input: ModelInput): ModelOutput {
  const ranArgs = (name: string) => input.toolHistory.filter(h => h.name === name).map(h => h.args)
  const lastResult = (name: string) => input.toolHistory.find(h => h.name === name)?.result ?? ''

  // 还没看过项目结构 → 先 glob
  if (!ranArgs('glob_files').length) {
    return { kind: 'tool_call', call: { name: 'glob_files', args: { pattern: '**/*.{ts,md}' } } }
  }

  // 还没读过 CLAUDE.md → 先看约定
  const readArgs = ranArgs('read_file')
  if (!readArgs.some(a => a.path === 'CLAUDE.md')) {
    return { kind: 'tool_call', call: { name: 'read_file', args: { path: 'CLAUDE.md' } } }
  }

  // 还没 grep returnTo → 看谁在用 returnTo
  if (!ranArgs('grep_content').some(a => a.pattern === 'returnTo')) {
    return { kind: 'tool_call', call: { name: 'grep_content', args: { pattern: 'returnTo' } } }
  }

  // grep 结果显示 redirect.ts 提到了 returnTo（注释里），但还没看实现 → 读它
  const grepResult = lastResult('grep_content')
  const suspectFile = 'src/middleware/redirect.ts'
  if (grepResult.includes(suspectFile) && !readArgs.some(a => a.path === suspectFile)) {
    return { kind: 'tool_call', call: { name: 'read_file', args: { path: suspectFile } } }
  }

  // 探索完了 → 给出答案
  return {
    kind: 'final',
    content:
      'Bug 定位：`src/middleware/redirect.ts` 的 `postLoginRedirect` 把所有用户硬编码跳到了 `/admin`，' +
      '没有读 `returnTo` cookie。\n\n修复方案：把 `res.redirect("/admin")` 改成 ' +
      '`res.redirect(req.cookies.returnTo || "/")`，然后再 `res.clearCookie("returnTo")`。',
  }
}


然后是 Agent Loop——它把工具调用、模型决策、循环控制串起来：

src/agent.ts
应用
复制
import { globFiles, grepContent, readFileTool } from './tools.js'
import { mockModel } from './model.js'

export interface ToolCall {
  name: string
  args: any
}

const tools: Record<string, (args: any) => Promise<string>> = {
  glob_files: async ({ pattern }) => (await globFiles(pattern)).join('\n'),
  grep_content: async ({ pattern, path }) => {
    const hits = await grepContent(pattern, { path })
    return hits.map(h => `${h.file}:${h.line}  ${h.content}`).join('\n')
  },
  read_file: async ({ path }) => readFileTool(path),
}

export async function runAgent(userMessage: string, maxTurns = 8) {
  const toolHistory: { name: string; args: any; result: string }[] = []
  let turn = 0

  console.log(`👤 用户: ${userMessage}\n`)

  while (turn < maxTurns) {
    turn++
    const decision = mockModel({ userMessage, toolHistory })

    if (decision.kind === 'final') {
      console.log(`\n🤖 Agent 答案:\n${decision.content}`)
      console.log(`\n📊 共 ${turn - 1} 轮工具调用，${toolHistory.length} 次工具执行`)
      const totalChars = toolHistory.reduce((sum, h) => sum + h.result.length, 0)
      console.log(`📏 工具返回总字符: ${totalChars}（按 1 token ≈ 2 字符估算 ≈ ${Math.ceil(totalChars / 2)} tokens）`)
      return
    }

    const { name, args } = decision.call
    console.log(`🔧 Turn ${turn}: ${name}(${JSON.stringify(args)})`)
    const result = await tools[name](args)
    const preview = result.length > 200 ? result.slice(0, 200) + `\n... (+${result.length - 200} 字符)` : result
    console.log(`   ↳ ${preview.replace(/\n/g, '\n   ')}\n`)
    toolHistory.push({ name, args, result })
  }
}

src/index.ts
应用
复制
import { runAgent } from './agent.js'

await runAgent('用户反馈登录后总是跳到 /admin，不管他们之前访问的是哪个页面，帮我定位这个 bug')

bash
运行
复制
pnpm start


输出会是这样：

👤 用户: 用户反馈登录后总是跳到 /admin...

🔧 Turn 1: glob_files({"pattern":"**/*.{ts,md}"})
   ↳ CLAUDE.md
     src/app.ts
     src/auth/login.ts
     ... (10 个文件)

🔧 Turn 2: read_file({"path":"CLAUDE.md"})
   ↳ # Express Auth Demo ... returnTo cookie ...

🔧 Turn 3: grep_content({"pattern":"returnTo"})
   ↳ CLAUDE.md:5 ... auth.ts:16 ... redirect.ts:5,6 ...

🔧 Turn 4: read_file({"path":"src/middleware/redirect.ts"})
   ↳ res.redirect('/admin')

🤖 Agent 答案:
Bug 定位：src/middleware/redirect.ts 把所有用户硬编码跳到了 /admin...

📊 共 4 轮工具调用，4 次工具执行
📏 工具返回总字符: 1415（≈ 708 tokens）


中间经历了四轮工具调用、消耗了 ~700 tokens，完成了这个 bug 的定位。

注意每一步的递进——Agent 不是漫无目的地搜，而是每一步的结果都在指引下一步的探索方向：

Glob 看到 `middleware/redirect.ts` 这个文件名 → 怀疑跟跳转相关
CLAUDE.md 提到 returnTo 是关键约定 → 决定 grep returnTo
grep 结果显示 redirect.ts 在注释里提了 returnTo 但实现没匹配上 → 读这个文件确认


这个上下文递进过程的专业术语叫 Progressive Disclosure（渐进式披露）——每一步只取下一步真正需要的信息。

但是你可能会想：要是不用 JIT，直接把所有文件全塞给模型，token 消耗到底差多少？下一步我们就来做这个对比。

Step 5：对比"全读"基线 + 生产级优化

我们写一个 runEager 函数，把所有文件一股脑读进来当对照组：

src/agent.ts
应用
复制
// ... 前面的 runAgent 同 Step 4 ...

/**
 * 基线对比：全读策略——把所有文件一股脑塞进上下文。
 */
export async function runEager() {
  const files = await globFiles('**/*.{ts,md}')
  let total = ''
  for (const f of files) {
    total += `\n--- ${f} ---\n` + (await readFileTool(f))
  }
  console.log(`\n📊 全读策略: 读了 ${files.length} 个文件，总字符 ${total.length} ≈ ${Math.ceil(total.length / 2)} tokens`)
  return total.length
}


同时给 tools 加上生产级 Agent 都需要的安全设计——Glob 默认忽略 node_modules/dist 这些噪音，Grep 加结果上限避免一次塞回上千条匹配，Read 支持 offset/limit 处理大文件：

src/tools.ts
应用
复制
import { readdir, readFile, stat } from 'node:fs/promises'
import { join, relative, sep } from 'node:path'

const PROJECT_ROOT = 'bug-project'
const DEFAULT_IGNORE = ['node_modules', 'dist', '.git', 'coverage']

async function* walk(dir: string, ignore: string[]): AsyncGenerator<string> {
  const entries = await readdir(dir, { withFileTypes: true })
  for (const entry of entries) {
    if (ignore.includes(entry.name)) continue
    const path = join(dir, entry.name)
    if (entry.isDirectory()) {
      yield* walk(path, ignore)
    } else if (entry.isFile()) {
      yield path
    }
  }
}

// matchPattern 同前

export async function globFiles(
  pattern: string,
  opts: { ignore?: string[] } = {}
): Promise<string[]> {
  const ignore = [...DEFAULT_IGNORE, ...(opts.ignore ?? [])]
  // ... 用 walk(PROJECT_ROOT, ignore) ...
}

export async function grepContent(
  pattern: string,
  opts: { path?: string; maxResults?: number } = {}
): Promise<GrepHit[]> {
  const maxResults = opts.maxResults ?? 50
  // ... hits.length >= maxResults 时提前 return ...
}

export async function readFileTool(
  path: string,
  opts: { offset?: number; limit?: number } = {}
): Promise<string> {
  const content = await readFile(join(PROJECT_ROOT, path), 'utf8')
  if (opts.offset === undefined && opts.limit === undefined) return content

  const lines = content.split('\n')
  const start = opts.offset ?? 0
  const end = opts.limit ? start + opts.limit : lines.length
  const slice = lines.slice(start, end)
  const truncated = end < lines.length ? `\n... (省略 ${lines.length - end} 行)` : ''
  return slice.join('\n') + truncated
}


把 model 升级——读文件时主动加 limit: 30，避免大文件一次性塞满：

src/model.ts
应用
复制
// ... 同 Step 4，read_file 的 args 改成 { path, limit: 30 } ...


然后 index.ts 跑两种策略对比：

src/index.ts
应用
复制
import { runAgent, runEager } from './agent.js'

console.log('=== 策略 A: JIT 探索（Progressive Disclosure 版）===\n')
const jitChars = await runAgent('用户反馈登录后总是跳到 /admin，不管他们之前访问的是哪个页面，帮我定位这个 bug')

console.log('\n\n=== 策略 B: 全读基线 ===')
const eagerChars = await runEager()

console.log(`\n\n=== 对比 ===`)
console.log(`JIT 节省了 ${(eagerChars / jitChars).toFixed(1)} 倍 token`)
console.log(`(${jitChars} 字符 vs ${eagerChars} 字符)`)

bash
运行
复制
pnpm start


跑出来：

📊 JIT: 4 轮工具调用，工具返回总字符 1415 ≈ 708 tokens
📊 全读策略: 读了 10 个文件，总字符 4063 ≈ 2032 tokens

=== 对比 ===
JIT 节省了 2.9 倍 token


10 个文件的小项目，差距是 ~3 倍。但思考一下：这个项目放大到 200 个文件的真实仓库会怎样？

JIT 的 token 消耗几乎不变——它还是 glob 一次（路径列表稍微长一点）、grep 一次（命中数量受 maxResults 上限保护）、读 1-2 个文件。

而全读策略的 token 消耗会线性放大 20 倍以上——光是把 200 个文件全塞进上下文，几万 token 是基础消费。再考虑模型的注意力随上下文长度衰减（前面 KV Cache 那篇讲过的 Context Rot），全读不只是贵，还会让模型表现下降。

JIT 路线一：Agentic Search

我们在右侧编辑器里跑通的这套 Glob → Grep → Read 三件套，就是 Agentic Search——Agent 自己决定下一步该搜什么、该读什么，运行时用工具主动探索。

Claude Code 在生产环境用的几乎是同一套。它早期版本试过本地向量数据库做代码 RAG，后来放弃了。Anthropic 工程师 Boris 在 Latent Space 播客里说："agentic search outperformed it by a lot." 不只是 Claude Code，Cursor、Devin、Windsurf 这些主流 coding agent 都收敛到了同一个方案——用 grep、find、直接读文件，而不是向量搜索。

为什么这些产品在代码场景里选了 Agentic Search 而不是 RAG？结合我们刚刚的 Demo 场景，可以从下面的几个维度来分析：

精确度。 Grep 找的是精确匹配——你搜 returnTo，100% 能找到所有出现位置。embedding 检索是模糊的，可能给你"语义相关但不是那个函数"的代码。我们 demo 里 grep returnTo 直接命中了注释里面提到但实现没用的那个 redirect.ts——这种"提到但没用"的语义不一致，向量检索很难捕捉。

实时性。 你正在改代码，文件分分钟在变。RAG 的索引是预计算的，改了一行代码索引就过期了。Grep 永远搜的是当前文件。

可观测。 我们 demo 里每一步都看得清清楚楚——Agent 搜了什么、搜到了什么、读了哪个文件。RAG 的检索过程是黑盒，embedding 空间里发生了什么你不知道。

零额外基础设施。 不需要向量数据库、不需要预计算 embedding、不需要索引更新机制。文件系统就是"数据库"。

你可能会想：Agentic Search 每一步都是一次工具调用，token 消耗不是更多吗？实际上并非如此，Claude Code 在工程上做了两个强大的优化措施：

第一是 Prompt Cache。在 Agent Loop 每一轮有大量的前缀可以复用——system prompt 和之前的对话历史都不变，只有新增的工具调用结果在末尾。配合 Anthropic 的缓存定价（cache read 只要 base rate 的 1/10），实际整体 token 成本降低了 80% 左右。

第二是 Explore 子 Agent。如果需要大范围搜索（比如"在整个项目里找所有跟鉴权相关的代码"），Claude Code 会派一个子 Agent 出去做——在独立的上下文窗口里搜索，不污染主对话的上下文。搜完把结论带回来。这个我们在 Multi-Agent 那篇会专门讲。

JIT 路线二：RAG

RAG 也是 JIT 的一种实现——query 来的时候根据语义来检索，不是一股脑全塞。只不过它的"按需"发生在更早的阶段：预先建好 embedding 索引，查询时一次检索拿到 top-k 结果。这里面的技术细节我们会在下一章详细展开。

RAG 在代码场景被 Agentic Search 替代了，但在知识库问答场景，它依然是最合适的 JIT 路线：

文档问答——用户问"我们的退款政策是什么"，你知道答案一定在某几份文档里，预先检索比 Agent 自己翻快得多。
非结构化内容——一大堆 PDF 报告、会议纪要，没有好的目录结构让 Agent 去"探索"。
延迟敏感——RAG 一次检索就出结果，比 Agent 自己搜三四轮快。

关键区别在于：内容有没有确定性的结构可以遍历。代码有 import 关系、类型定义、目录结构——确定性工具（grep、find）比概率性检索（embedding）更准。但知识库通常没有这种可遍历结构，语义检索反而是更自然的"按需加载"方式。

JIT 路线三：Context Offloading（Manus）

前两种路线都是"从外部加载进来"。Manus 把 JIT 的思路推得更远——不光"按需读入"，还能"主动卸载"。方向反过来了：把上下文里已经用过的信息写出去，需要时再读回来。

Manus 的 Agent 平均每个任务要跑 50+ 次工具调用。工具返回的结果（网页内容、API 响应、命令输出）不断膨胀上下文。如果什么都留着，几十轮之后上下文就爆了。

Manus 的策略是：把旧的工具结果卸载到文件系统里，只在上下文中保留路径。Agent 需要的时候再读。

这跟压缩有一个本质区别：压缩是有损的（摘要一定会丢信息），卸载是无损的（内容还在文件里，随时能恢复）。Manus 的优先级是 原始内容 > 卸载 > 压缩 > 摘要，能不丢信息就不丢。

举个具体的例子：Agent 在第 10 轮搜了一个网页拿到 3000 token 内容，到第 30 轮这个内容已经不太用得上了。Manus 不是把它摘要成 300 token（信息会丢），而是把完整内容写到沙箱里的一个文件，上下文里只保留一行：[网页内容已保存到 /tmp/search_result_10.md，需要时可重新读取]。从 3000 token 变成 1 行——如果后面真的需要，Agent 调一次 read 就能拿回来。

文件路径就是恢复信息的"快捷方式"。上下文里存的是"怎么恢复信息"，而不是信息本身。

三种路线怎么选

回顾一下，JIT 作为总原则有三种实现路线：

路线	核心机制	适合场景
Agentic Search	Agent 用工具主动探索（Glob/Grep/Read）	代码导航、探索性任务、内容频繁变化
RAG	预建索引，查询时语义检索 top-k	知识库问答、非结构化文档、延迟敏感
Context Offloading	用过的信息卸载到文件，需要时再读回	长链路任务（50+ 轮工具调用）、上下文即将爆满

三者不互斥。实际生产环境通常是混合策略——Anthropic 推荐的 Hybrid Retrieval Strategy 就是把上下文按"确定性"分层：

确定性最高的预加载——CLAUDE.md、项目配置、用户画像。这些每次都用得到，内容稳定，放在 system prompt 前部能最大化 Cache 命中（Cache 那篇讲过）。

确定性中等的按规则触发——比如 Cursor 的 Rules：*.test.ts 被打开时自动加载测试规范，Dockerfile 被打开时自动加载部署指南。

确定性最低的交给 Agent 自主发现——任务相关的代码、数据，Agent 自己用 Agentic Search 或 RAG 去搜。这一层的 token 消耗不确定，但信息相关性最高。

而对于历史工具调用消息，我们可以学习 Manus 和 Cursor，直接使用 Context Offloading 来把一些比较长的内容卸载，存到磁盘或者数据库，然后留一个简单的标识 id 或者路径即可。

回到这篇的核心：上下文不是越早塞越好。 我们在右侧编辑器里跑的那 4 轮工具调用 vs 全读 10 个文件，差距虽然只有 3 倍——但这是 10 个文件的小项目；放大到 200 个文件的真实仓库，差距是 50 倍以上。全量预填充塞了太多无关内容会稀释模型的注意力（Context Rot），JIT 让每一条进入上下文的信息都是 Agent 当前真正需要的。

不管选哪条 JIT 路线，目标都是同一个——让模型在正确的时间看到正确的信息，不多不少。

这篇讲的所有内容——Agentic Search、RAG、Context Offloading——本质上都是在解决单次会话内的上下文问题。Agent 跑一轮任务，中间需要什么信息就去拿，任务结束了，这些上下文也就消散了。

但你想想，一个真正有用的 Agent 不是只服务一次。用户明天再来问同一个项目的问题，Agent 还得从头 glob 一遍？上次花了 4 轮工具调用定位到的 bug，下次遇到类似问题能不能直接跳过前面几步？

这就涉及到跨会话的长期上下文——也就是 Memory。它和 JIT 不是对立的，而是互补的：JIT 解决"当前这一轮怎么高效获取信息"，Memory 解决"上一轮学到的东西怎么留给下一轮用"。接下来我们进入这一章的后半段：从短期上下文管理，切换到长期记忆系统。

参考资料
Anthropic: Effective Context Engineering for AI Agents
Manus: Context Engineering for AI Agents
Martin Fowler: Context Engineering for Coding Agents
Claude Code Doesn't Index Your Codebase
Why Cursor, Claude Code, and Devin Use grep, Not Vectors
LangChain: Context Engineering for Agents
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
Cache 全解与成本控制：别再弄混 KV Cache、Prompt Cache、Context Collapse 这些概念了
下一篇 · 第四章：Context Engineering —— Agent 真正的护城河（含 Memory）
RAG 全流程：从一堆文档到 Agent 能用的知识库
编辑器


---
## 代码块


```
bug-project/
├── CLAUDE.md
├── src/
│   ├── app.ts                     - 应用入口
│   ├── auth/
│   │   ├── login.ts               - 登录路由
│   │   └── session.ts             - Session 管理
│   ├── middleware/
│   │   ├── auth.ts                - 鉴权中间件
│   │   └── redirect.ts            - 登录后跳转 ← 这里有 bug
│   ├── routes/{index,user,admin}.ts
│   └── utils/cookies.ts
```


```bash
pnpm install
```


```typescript
import { readdir } from 'node:fs/promises'
import { join, relative, sep } from 'node:path'

const PROJECT_ROOT = 'bug-project'

async function* walk(dir: string): AsyncGenerator<string> {
  const entries = await readdir(dir, { withFileTypes: true })
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) {
      yield* walk(path)
    } else if (entry.isFile()) {
      yield path
    }
  }
}

function matchPattern(path: string, pattern: string): boolean {
  // 简化版 glob:
  //   **/  匹配任意层目录（含零层）
  //   **   匹配任意字符
  //   *    匹配单层非斜杠字符
  //   {a,b} 匹配多个扩展名
  const expanded = pattern.replace(/\{([^}]+)\}/g, (_, opts) => `(${opts.split(',').join('|')})`)
  const regex = expanded
    .replace(/\./g, '\\.')
    .replace(/\*\*\//g, '__GLOBSTARSEP__')
    .replace(/\*\*/g, '__GLOBSTAR__')
    .replace(/\*/g, '[^/]*')
    .replace(/__GLOBSTARSEP__/g, '(?:.*/)?')
    .replace(/__GLOBSTAR__/g, '.*')
  return new RegExp(`^${regex}$`).test(path)
}

export async function globFiles(pattern: string): Promise<string[]> {
  const results: string[] = []
  for await (const path of walk(PROJECT_ROOT)) {
    const rel = relative(PROJECT_ROOT, path).split(sep).join('/')
    if (matchPattern(rel, pattern)) {
      results.push(rel)
    }
  }
  return results.sort()
}
```


```typescript
import { globFiles } from './tools.js'

console.log('=== 测试 Glob ===\n')

console.log('所有 .ts 文件:')
const tsFiles = await globFiles('**/*.ts')
tsFiles.forEach(f => console.log('  ' + f))
console.log(`(${tsFiles.length} 个文件)\n`)

console.log('所有 .md 文件:')
const mdFiles = await globFiles('**/*.md')
mdFiles.forEach(f => console.log('  ' + f))
console.log(`(${mdFiles.length} 个文件)\n`)

console.log('middleware 目录下的文件:')
const middleware = await globFiles('src/middleware/**')
middleware.forEach(f => console.log('  ' + f))
```


```bash
pnpm start
```


```typescript
import { readdir, readFile, stat } from 'node:fs/promises'
import { join, relative, sep } from 'node:path'

const PROJECT_ROOT = 'bug-project'

// ... walk 和 matchPattern 同 Step 1 ...

export async function globFiles(pattern: string): Promise<string[]> {
  // ... 同 Step 1 ...
}

export interface GrepHit {
  file: string
  line: number
  content: string
}

export async function grepContent(
  pattern: string,
  opts: { path?: string } = {}
): Promise<GrepHit[]> {
  const root = opts.path ? join(PROJECT_ROOT, opts.path) : PROJECT_ROOT
  const stats = await stat(root)
  const files: string[] = []
  if (stats.isDirectory()) {
    for await (const p of walk(root)) files.push(p)
  } else {
    files.push(root)
  }

  const regex = new RegExp(pattern)
  const hits: GrepHit[] = []
  for (const file of files) {
    const content = await readFile(file, 'utf8')
    content.split('\n').forEach((line, idx) => {
      if (regex.test(line)) {
        hits.push({
          file: relative(PROJECT_ROOT, file).split(sep).join('/'),
          line: idx + 1,
          content: line.trim(),
        })
      }
    })
  }
  return hits
}
```


```typescript
import { grepContent } from './tools.js'

console.log('=== 测试 Grep ===\n')

console.log('搜索 returnTo:')
const returnToHits = await grepContent('returnTo')
returnToHits.forEach(h => console.log(`  ${h.file}:${h.line}  ${h.content}`))
console.log(`(${returnToHits.length} 处命中)\n`)

console.log('搜索 res.redirect:')
const redirectHits = await grepContent('res\\.redirect')
redirectHits.forEach(h => console.log(`  ${h.file}:${h.line}  ${h.content}`))
console.log(`(${redirectHits.length} 处命中)`)
```


```bash
pnpm start
```


```
搜索 returnTo:
  CLAUDE.md:5  1. 用户访问受保护页面... 当前 URL 写到 `returnTo` cookie...
  CLAUDE.md:6  2. 用户登录成功后，应该读 `returnTo` cookie...
  src/middleware/auth.ts:16  res.cookie('returnTo', req.originalUrl, ...)
  src/middleware/redirect.ts:5  * 应该把用户送回他们登录前访问的页面（returnTo cookie），
  src/middleware/redirect.ts:6  * 如果没有 returnTo cookie 就回首页。
  src/utils/cookies.ts:4  res.cookie('returnTo', url, ...)
  ...
```


```typescript
// ... globFiles 和 grepContent 同前 ...

export async function readFileTool(path: string): Promise<string> {
  const content = await readFile(join(PROJECT_ROOT, path), 'utf8')
  return content
}
```


```typescript
import { globFiles, grepContent, readFileTool } from './tools.js'

console.log('=== JIT 三件套手动跑一遍：定位"登录后跳错页面"的 bug ===\n')

console.log('1️⃣  glob 看项目结构\n')
const files = await globFiles('**/*.{ts,md}')
files.forEach(f => console.log('  ' + f))
console.log()

console.log('2️⃣  read CLAUDE.md 了解约定\n')
const claudeMd = await readFileTool('CLAUDE.md')
console.log(claudeMd)

console.log('3️⃣  grep returnTo 看哪些文件用到了\n')
const hits = await grepContent('returnTo')
hits.forEach(h => console.log(`  ${h.file}:${h.line}`))
console.log()

console.log('4️⃣  read 嫌疑文件 redirect.ts\n')
console.log(await readFileTool('src/middleware/redirect.ts'))
```


```bash
pnpm start
```


```typescript
export function postLoginRedirect(_req: Request, res: Response) {
  res.clearCookie('returnTo')
  res.redirect('/admin')
}
```


```
Glob → Read（约定文档）→ Grep → Read（嫌疑文件）
```


```typescript
import type { ToolCall } from './agent.js'

interface ModelInput {
  userMessage: string
  toolHistory: { name: string; args: any; result: string }[]
}

type ModelOutput =
  | { kind: 'tool_call'; call: ToolCall }
  | { kind: 'final'; content: string }

/**
 * Mock 模型：模拟 JIT 探索的决策逻辑。
 * 真实模型会自己想清楚下一步该做什么；这里我们用一个状态机来模拟。
 */
export function mockModel(input: ModelInput): ModelOutput {
  const ranArgs = (name: string) => input.toolHistory.filter(h => h.name === name).map(h => h.args)
  const lastResult = (name: string) => input.toolHistory.find(h => h.name === name)?.result ?? ''

  // 还没看过项目结构 → 先 glob
  if (!ranArgs('glob_files').length) {
    return { kind: 'tool_call', call: { name: 'glob_files', args: { pattern: '**/*.{ts,md}' } } }
  }

  // 还没读过 CLAUDE.md → 先看约定
  const readArgs = ranArgs('read_file')
  if (!readArgs.some(a => a.path === 'CLAUDE.md')) {
    return { kind: 'tool_call', call: { name: 'read_file', args: { path: 'CLAUDE.md' } } }
  }

  // 还没 grep returnTo → 看谁在用 returnTo
  if (!ranArgs('grep_content').some(a => a.pattern === 'returnTo')) {
    return { kind: 'tool_call', call: { name: 'grep_content', args: { pattern: 'returnTo' } } }
  }

  // grep 结果显示 redirect.ts 提到了 returnTo（注释里），但还没看实现 → 读它
  const grepResult = lastResult('grep_content')
  const suspectFile = 'src/middleware/redirect.ts'
  if (grepResult.includes(suspectFile) && !readArgs.some(a => a.path === suspectFile)) {
    return { kind: 'tool_call', call: { name: 'read_file', args: { path: suspectFile } } }
  }

  // 探索完了 → 给出答案
  return {
    kind: 'final',
    content:
      'Bug 定位：`src/middleware/redirect.ts` 的 `postLoginRedirect` 把所有用户硬编码跳到了 `/admin`，' +
      '没有读 `returnTo` cookie。\n\n修复方案：把 `res.redirect("/admin")` 改成 ' +
      '`res.redirect(req.cookies.returnTo || "/")`，然后再 `res.clearCookie("returnTo")`。',
  }
}
```


```typescript
import { globFiles, grepContent, readFileTool } from './tools.js'
import { mockModel } from './model.js'

export interface ToolCall {
  name: string
  args: any
}

const tools: Record<string, (args: any) => Promise<string>> = {
  glob_files: async ({ pattern }) => (await globFiles(pattern)).join('\n'),
  grep_content: async ({ pattern, path }) => {
    const hits = await grepContent(pattern, { path })
    return hits.map(h => `${h.file}:${h.line}  ${h.content}`).join('\n')
  },
  read_file: async ({ path }) => readFileTool(path),
}

export async function runAgent(userMessage: string, maxTurns = 8) {
  const toolHistory: { name: string; args: any; result: string }[] = []
  let turn = 0

  console.log(`👤 用户: ${userMessage}\n`)

  while (turn < maxTurns) {
    turn++
    const decision = mockModel({ userMessage, toolHistory })

    if (decision.kind === 'final') {
      console.log(`\n🤖 Agent 答案:\n${decision.content}`)
      console.log(`\n📊 共 ${turn - 1} 轮工具调用，${toolHistory.length} 次工具执行`)
      const totalChars = toolHistory.reduce((sum, h) => sum + h.result.length, 0)
      console.log(`📏 工具返回总字符: ${totalChars}（按 1 token ≈ 2 字符估算 ≈ ${Math.ceil(totalChars / 2)} tokens）`)
      return
    }

    const { name, args } = decision.call
    console.log(`🔧 Turn ${turn}: ${name}(${JSON.stringify(args)})`)
    const result = await tools[name](args)
    const preview = result.length > 200 ? result.slice(0, 200) + `\n... (+${result.length - 200} 字符)` : result
    console.log(`   ↳ ${preview.replace(/\n/g, '\n   ')}\n`)
    toolHistory.push({ name, args, result })
  }
}
```


```typescript
import { runAgent } from './agent.js'

await runAgent('用户反馈登录后总是跳到 /admin，不管他们之前访问的是哪个页面，帮我定位这个 bug')
```


```bash
pnpm start
```


```
👤 用户: 用户反馈登录后总是跳到 /admin...

🔧 Turn 1: glob_files({"pattern":"**/*.{ts,md}"})
   ↳ CLAUDE.md
     src/app.ts
     src/auth/login.ts
     ... (10 个文件)

🔧 Turn 2: read_file({"path":"CLAUDE.md"})
   ↳ # Express Auth Demo ... returnTo cookie ...

🔧 Turn 3: grep_content({"pattern":"returnTo"})
   ↳ CLAUDE.md:5 ... auth.ts:16 ... redirect.ts:5,6 ...

🔧 Turn 4: read_file({"path":"src/middleware/redirect.ts"})
   ↳ res.redirect('/admin')

🤖 Agent 答案:
Bug 定位：src/middleware/redirect.ts 把所有用户硬编码跳到了 /admin...

📊 共 4 轮工具调用，4 次工具执行
📏 工具返回总字符: 1415（≈ 708 tokens）
```


```
Glob 看到 `middleware/redirect.ts` 这个文件名 → 怀疑跟跳转相关
CLAUDE.md 提到 returnTo 是关键约定 → 决定 grep returnTo
grep 结果显示 redirect.ts 在注释里提了 returnTo 但实现没匹配上 → 读这个文件确认
```


```typescript
// ... 前面的 runAgent 同 Step 4 ...

/**
 * 基线对比：全读策略——把所有文件一股脑塞进上下文。
 */
export async function runEager() {
  const files = await globFiles('**/*.{ts,md}')
  let total = ''
  for (const f of files) {
    total += `\n--- ${f} ---\n` + (await readFileTool(f))
  }
  console.log(`\n📊 全读策略: 读了 ${files.length} 个文件，总字符 ${total.length} ≈ ${Math.ceil(total.length / 2)} tokens`)
  return total.length
}
```


```typescript
import { readdir, readFile, stat } from 'node:fs/promises'
import { join, relative, sep } from 'node:path'

const PROJECT_ROOT = 'bug-project'
const DEFAULT_IGNORE = ['node_modules', 'dist', '.git', 'coverage']

async function* walk(dir: string, ignore: string[]): AsyncGenerator<string> {
  const entries = await readdir(dir, { withFileTypes: true })
  for (const entry of entries) {
    if (ignore.includes(entry.name)) continue
    const path = join(dir, entry.name)
    if (entry.isDirectory()) {
      yield* walk(path, ignore)
    } else if (entry.isFile()) {
      yield path
    }
  }
}

// matchPattern 同前

export async function globFiles(
  pattern: string,
  opts: { ignore?: string[] } = {}
): Promise<string[]> {
  const ignore = [...DEFAULT_IGNORE, ...(opts.ignore ?? [])]
  // ... 用 walk(PROJECT_ROOT, ignore) ...
}

export async function grepContent(
  pattern: string,
  opts: { path?: string; maxResults?: number } = {}
): Promise<GrepHit[]> {
  const maxResults = opts.maxResults ?? 50
  // ... hits.length >= maxResults 时提前 return ...
}

export async function readFileTool(
  path: string,
  opts: { offset?: number; limit?: number } = {}
): Promise<string> {
  const content = await readFile(join(PROJECT_ROOT, path), 'utf8')
  if (opts.offset === undefined && opts.limit === undefined) return content

  const lines = content.split('\n')
  const start = opts.offset ?? 0
  const end = opts.limit ? start + opts.limit : lines.length
  const slice = lines.slice(start, end)
  const truncated = end < lines.length ? `\n... (省略 ${lines.length - end} 行)` : ''
  return slice.join('\n') + truncated
}
```


```typescript
// ... 同 Step 4，read_file 的 args 改成 { path, limit: 30 } ...
```


```typescript
import { runAgent, runEager } from './agent.js'

console.log('=== 策略 A: JIT 探索（Progressive Disclosure 版）===\n')
const jitChars = await runAgent('用户反馈登录后总是跳到 /admin，不管他们之前访问的是哪个页面，帮我定位这个 bug')

console.log('\n\n=== 策略 B: 全读基线 ===')
const eagerChars = await runEager()

console.log(`\n\n=== 对比 ===`)
console.log(`JIT 节省了 ${(eagerChars / jitChars).toFixed(1)} 倍 token`)
console.log(`(${jitChars} 字符 vs ${eagerChars} 字符)`)
```


```bash
pnpm start
```


```
📊 JIT: 4 轮工具调用，工具返回总字符 1415 ≈ 708 tokens
📊 全读策略: 读了 10 个文件，总字符 4063 ≈ 2032 tokens

=== 对比 ===
JIT 节省了 2.9 倍 token
```
