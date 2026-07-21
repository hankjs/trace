# Agent 不能这么脆——循环检测、API 容错与 Token 预算

课程
Super Agent 实战课
Agent 不能这么脆——循环检测、API 容错与 Token 预算
Agent 不能这么脆——循环检测、API 容错与 Token 预算

约 57 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇结束的时候，你的 Agent 已经能调工具了——查天气、算数学、多步推理，看起来挺像回事。

但说实话，现在这个 Agent 其实挺脆的。

你让它帮你查个文件，它 read_file 读完发现路径不对，于是它决定……再读一次。同样的路径，同样的参数，同样的错误结果。然后它"恍然大悟"——再试一次。于是你看着它在终端里一直输出：

--- Step 1 ---
  [调用: read_file({"path":"/tmp/data.csv"})]
  [结果: "文件不存在"]

--- Step 2 ---
  [调用: read_file({"path":"/tmp/data.csv"})]
  [结果: "文件不存在"]

--- Step 3 ---
  [调用: read_file({"path":"/tmp/data.csv"})]
  [结果: "文件不存在"]

...（一直到 Step 10，MAX_STEPS 救了你一命）


10 步到了，强制停了。但如果你把 MAX_STEPS 设得大一点呢？比如 50？100？200？

这不是理论。在生产环境里，一个不受控的 Agent 跑 200 轮，每轮上下文越滚越大，token 消耗是指数级增长的。用户可能根本不知道——他去倒了杯水回来，Agent 还在转圈，账单已经飙上去了。

所以生产级 Agent 需要防护机制，而且不是一层，得有多层。

先把依赖装上，后面每一步写完代码都能直接跑：

bash
运行
复制
pnpm install

三层防护，逐层接入

这篇我们给 Agent Loop 逐层加三道防护——每加一层，跑一次，亲眼看效果：

循环检测——模型反复做同样的事且没有进展，检测到并打断它
API 容错——API 限流、超时、网络断开，自动重试而不是直接崩
Token 预算——累计追踪 token 消耗，超预算自动停止

你可以把它们类比成家里配电箱的三种保护：循环检测是短路保护，API 容错是过载保护，Token 预算是漏电保护。各管各的，互不干扰，但少了任何一个都不踏实。

准备工作：入口文件 + Mock Model

在加防护之前，先把基础设施搭好。src/index.ts 跟上一篇结构一样，只是换成调 agentLoop()：

src/index.ts
应用
复制
import 'dotenv/config';
import { type ModelMessage } from 'ai';
import { createOpenAI } from '@ai-sdk/openai';
import { createMockModel } from './mock-model.js';
import { createInterface } from 'node:readline';
import { weatherTool, calculatorTool } from './tools.js';
import { agentLoop, type BudgetState } from './agent-loop.js';

const qwen = createOpenAI({
  baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
  apiKey: process.env.DASHSCOPE_API_KEY,
});

const model = process.env.DASHSCOPE_API_KEY
  ? qwen.chat('qwen-plus-latest')
  : createMockModel();

const tools = { get_weather: weatherTool, calculator: calculatorTool };
const messages: ModelMessage[] = [];
// 预算由调用方持有，跨轮持续累计——agentLoop 只负责消费它
const budget: BudgetState = { used: 0, limit: 15000 };
const rl = createInterface({ input: process.stdin, output: process.stdout });

const SYSTEM = `你是 Super Agent，一个有工具调用能力的 AI 助手。
需要查询信息时，主动使用工具，不要编造数据。
回答要简洁直接。`;

function ask() {
  rl.question('\nYou: ', async (input) => {
    const trimmed = input.trim();
    if (!trimmed || trimmed === 'exit') {
      console.log('Bye!');
      rl.close();
      return;
    }

    messages.push({ role: 'user', content: trimmed });

    await agentLoop(model, tools, messages, SYSTEM, budget);

    ask();
  });
}

console.log('Super Agent v0.3 — Fuses (type "exit" to quit)\n');
console.log('试试输入："测试死循环"、"测试重试"、"测试预算" 看三层防护效果\n');
ask();


注意 budget 声明在模块顶层，跨多轮 user 提问持续累计。如果把 let totalTokens = 0 写在 agentLoop 函数内部，每次新一轮 query 重新进入函数，累计会被清零——这是个隐蔽的 bug，生产环境下你以为加了预算，其实每轮都从 0 开始，根本兜不住长会话的滚雪球。把状态归调用方持有，语义就清晰了：agentLoop 只是消费 budget，不拥有它。

Mock Model 也需要升级——支持模拟"死循环"和"API 错误"两种场景：

src/mock-model.ts
应用
复制
// ... mock model 核心结构同上一篇 ...
// 新增：死循环模拟 + API 错误模拟

// 在 detectToolIntent 中新增：
// 1. 用户说"测试死循环" → 每次都返回 get_weather({"city":"北京"})
// 2. 用户说"测试重试" → 抛出 429 错误


应用代码后，完整的 mock model 会自动配置好这些场景。

第一层：循环检测
语义层面的死循环

上一篇用 MAX_STEPS 兜住了代码层面的无限循环。但你想想，真正麻烦的不是 while(true)，而是模型在不断地做事，但没有任何进展。它每一步都在调工具，看起来很忙，但其实在原地打转。

最常见的三种模式：

通用重复——同一个工具、同样的参数、同样的结果，反复调
乒乓循环——两个操作来回交替，A → B → A → B，每一步看起来都在"做事"，但整体没有进展
轮询无进展——不断 poll 检查状态，结果一直是 "running"
核心思路：指纹 + 滑动窗口

检测这些模式的思路其实不复杂：

给每次工具调用算指纹——把工具名 + 参数做一次确定性的 JSON 序列化（key 排序），然后哈希。这样 get_weather({"city":"北京"}) 不管参数顺序怎么变，指纹都一样
维护滑动窗口（最近 30 条）——只看最近的行为，早期的正常行为不太具备参考意义，主要看看最近若干轮有没有出现重复。
同样的输入 + 同样的输出 = 无进展——光看参数相同还不够。模型调了 10 次 read_file 但每次读的都是不同文件，这是正常探索。只有调用指纹和结果指纹都一样，才算真的没进展。

检测到重复后不是一刀切，而是三级响应：

级别	阈值	行为
Warning	5 次	注入系统提醒消息，让模型"醒过来"换策略
Critical	8 次	阻断工具调用，强制停止循环
全局熔断	10 次	无论什么情况，强制停止

为什么不在第一次重复就停？我觉得这里有个很重要的取舍：误杀的代价太大。把一个正在正常工作的 Agent 强行停了，比让它多跑几轮更浪费。先软后硬，给模型自救的机会。

创建 src/loop-detection.ts：

src/loop-detection.ts
应用
复制
import { createHash } from 'node:crypto';

// --- 类型定义 ---

export interface ToolCallRecord {
  toolName: string;
  argsHash: string;
  resultHash?: string;
  timestamp: number;
}

export type DetectorKind = 'generic_repeat' | 'ping_pong' | 'global_circuit_breaker';

export type DetectionResult =
  | { stuck: false }
  | { stuck: true; level: 'warning' | 'critical'; detector: DetectorKind; count: number; message: string };

// --- 配置 ---

const HISTORY_SIZE = 30;       // 滑动窗口大小
const WARNING_THRESHOLD = 5;   // 警告阈值（演示用，生产环境通常是 10）
const CRITICAL_THRESHOLD = 8;  // 严重阈值（演示用，生产环境通常是 20）
const BREAKER_THRESHOLD = 10;  // 熔断阈值（演示用，生产环境通常是 30）

// --- 指纹计算 ---

function stableStringify(value: unknown): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(',')}]`;
  const keys = Object.keys(value as Record<string, unknown>).sort();
  return `{${keys.map(k => `${JSON.stringify(k)}:${stableStringify((value as any)[k])}`).join(',')}}`;
}

function hash(input: string): string {
  return createHash('sha256').update(input).digest('hex').slice(0, 16);
}

export function hashToolCall(toolName: string, params: unknown): string {
  return `${toolName}:${hash(stableStringify(params))}`;
}

export function hashResult(result: unknown): string {
  return hash(stableStringify(result));
}

// --- 滑动窗口 ---

const history: ToolCallRecord[] = [];

export function recordCall(toolName: string, params: unknown): void {
  history.push({
    toolName,
    argsHash: hashToolCall(toolName, params),
    timestamp: Date.now(),
  });
  if (history.length > HISTORY_SIZE) history.shift();
}

export function recordResult(toolName: string, params: unknown, result: unknown): void {
  const argsHash = hashToolCall(toolName, params);
  const resultH = hashResult(result);
  for (let i = history.length - 1; i >= 0; i--) {
    if (history[i].toolName === toolName && history[i].argsHash === argsHash && !history[i].resultHash) {
      history[i].resultHash = resultH;
      break;
    }
  }
}

export function resetHistory(): void {
  history.length = 0;
}

// --- 检测器 ---

function getNoProgressStreak(toolName: string, argsHash: string): number {
  let streak = 0;
  let lastResultHash: string | undefined;
  for (let i = history.length - 1; i >= 0; i--) {
    const r = history[i];
    if (r.toolName !== toolName || r.argsHash !== argsHash) continue;
    if (!r.resultHash) continue;
    if (!lastResultHash) { lastResultHash = r.resultHash; streak = 1; continue; }
    if (r.resultHash !== lastResultHash) break;
    streak++;
  }
  return streak;
}

function getPingPongCount(currentHash: string): number {
  if (history.length < 3) return 0;
  const last = history[history.length - 1];
  let otherHash: string | undefined;
  for (let i = history.length - 2; i >= 0; i--) {
    if (history[i].argsHash !== last.argsHash) { otherHash = history[i].argsHash; break; }
  }
  if (!otherHash) return 0;
  let count = 0;
  for (let i = history.length - 1; i >= 0; i--) {
    const expected = count % 2 === 0 ? last.argsHash : otherHash;
    if (history[i].argsHash !== expected) break;
    count++;
  }
  if (currentHash === otherHash && count >= 2) return count + 1;
  return 0;
}

// --- 主检测函数 ---

export function detect(toolName: string, params: unknown): DetectionResult {
  const argsHash = hashToolCall(toolName, params);
  const noProgress = getNoProgressStreak(toolName, argsHash);

  if (noProgress >= BREAKER_THRESHOLD) {
    return { stuck: true, level: 'critical', detector: 'global_circuit_breaker', count: noProgress,
      message: `[熔断] ${toolName} 已重复 ${noProgress} 次且无进展，强制停止` };
  }

  const pingPong = getPingPongCount(argsHash);
  if (pingPong >= CRITICAL_THRESHOLD) {
    return { stuck: true, level: 'critical', detector: 'ping_pong', count: pingPong,
      message: `[熔断] 检测到乒乓循环（${pingPong} 次交替），强制停止` };
  }
  if (pingPong >= WARNING_THRESHOLD) {
    return { stuck: true, level: 'warning', detector: 'ping_pong', count: pingPong,
      message: `[警告] 检测到乒乓循环（${pingPong} 次交替），建议换个思路` };
  }

  const recentCount = history.filter(h => h.toolName === toolName && h.argsHash === argsHash).length;
  if (recentCount >= CRITICAL_THRESHOLD) {
    return { stuck: true, level: 'critical', detector: 'generic_repeat', count: recentCount,
      message: `[熔断] ${toolName} 相同参数已调用 ${recentCount} 次，强制停止` };
  }
  if (recentCount >= WARNING_THRESHOLD) {
    return { stuck: true, level: 'warning', detector: 'generic_repeat', count: recentCount,
      message: `[警告] ${toolName} 相同参数已调用 ${recentCount} 次，你可能陷入了重复` };
  }

  return { stuck: false };
}

接进 Agent Loop

接下来把循环检测接进 agent-loop.ts。改动不大，就是在 tool-call 事件里加一个 detect() 检查：

src/agent-loop.ts
应用
复制
import { streamText, type ModelMessage } from 'ai';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';

const MAX_STEPS = 15;

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
) {
  let step = 0;
  resetHistory();

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    const result = await streamText({ model, system, tools, messages, maxRetries: 0, onError: () => {} });

    let hasToolCall = false;
    let fullText = '';
    let shouldBreak = false;
    let lastToolCall: { name: string; input: unknown } | null = null;

    for await (const part of result.fullStream) {
      switch (part.type) {
        case 'text-delta':
          process.stdout.write(part.text);
          fullText += part.text;
          break;

        case 'tool-call': {
          hasToolCall = true;
          lastToolCall = { name: part.toolName, input: part.input };
          console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);

          const detection = detect(part.toolName, part.input);
          if (detection.stuck) {
            console.log(`  ${detection.message}`);
            if (detection.level === 'critical') {
              shouldBreak = true;
            } else {
              messages.push({
                role: 'user' as const,
                content: `[系统提醒] ${detection.message}。请换一个思路解决问题，不要重复同样的操作。`,
              });
            }
          }
          recordCall(part.toolName, part.input);
          break;
        }

        case 'tool-result':
          console.log(`  [结果: ${JSON.stringify(part.output)}]`);
          if (lastToolCall) {
            recordResult(lastToolCall.name, lastToolCall.input, part.output);
          }
          break;
      }
    }

    if (shouldBreak) {
      console.log('\n[循环检测触发，Agent 已停止]');
      break;
    }

    const stepResult = await result.response;
    messages.push(...stepResult.messages);

    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    console.log('  → 继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}


跑起来验证：

bash
运行
复制
pnpm start


右侧 WebContainer 终端对退格（Backspace）键的支持有限，输入时建议一次打完再回车。本地终端没有这个问题。

先试「北京天气怎么样」确认正常功能没受影响，然后输入「测试死循环」看看防护效果：

You: 测试死循环

--- Step 1 ---
  [调用: get_weather({"city":"北京"})]
  [结果: "晴，15-25°C，东南风 2 级"]
  → 继续下一步...

--- Step 2 ~ 5 ---
  （同样的调用重复 4 次...）

--- Step 6 ---
  [调用: get_weather({"city":"北京"})]
  [警告] get_weather 相同参数已调用 5 次，你可能陷入了重复
  [结果: "晴，15-25°C，东南风 2 级"]
  → 继续下一步...

--- Step 7 ---
根据查询结果：晴，15-25°C，东南风 2 级


看到了吗？Warning 在第 6 步触发（历史中已有 5 次相同调用），注入了一条系统提醒消息。Mock model 收到提醒后"醒过来"了，第 7 步不再调工具，直接输出文本结束。

这就是三级响应的价值。实际上真实模型在收到 Warning 注入后，大部分情况都能自己调整策略。但如果遇到不听劝的，到 8 次时 Critical 触发，直接熔断强制停止。生产环境阈值通常更高（Warning 10、Critical 20、全局熔断 30），这里为了让你快速看到效果调低了。

第二层：API 容错

讲真，API 出错这事在生产环境太常见了。但不能一个 try-catch 一兜了事——429 限流等一会就好，400 参数错误重试一万次也没用。核心是分类：哪些值得重试，哪些直接抛。

创建 src/retry.ts：

src/retry.ts
应用
复制
// --- 错误分类 ---

export function isRetryable(error: unknown): boolean {
  if (!(error instanceof Error)) return false;

  const message = error.message || '';

  // HTTP 状态码判断
  const statusMatch = message.match(/(\d{3})/);
  if (statusMatch) {
    const status = parseInt(statusMatch[1]);
    if ([429, 529, 408].includes(status)) return true;
    if (status >= 500 && status < 600) return true;
    if (status >= 400 && status < 500) return false;
  }

  // 网络错误
  if (message.includes('ECONNRESET') || message.includes('EPIPE')) return true;
  if (message.includes('ETIMEDOUT') || message.includes('timeout')) return true;
  if (message.includes('fetch failed') || message.includes('network')) return true;
  // AI SDK 会把流式错误包装成 NoOutputGeneratedError
  if (message.includes('No output generated')) return true;

  return false;
}

// --- 指数退避 + 随机抖动 ---

export function calculateDelay(attempt: number, baseMs = 500, maxMs = 30000): number {
  const exponential = baseMs * Math.pow(2, attempt - 1);
  const capped = Math.min(exponential, maxMs);
  const jitterRange = capped * 0.25;
  const jittered = capped + (Math.random() * 2 - 1) * jitterRange;
  return Math.max(0, Math.round(jittered));
}

export function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

指数退避为什么要加抖动

calculateDelay 里有两个关键设计：

指数退避：每次重试等的时间翻倍——500ms → 1000ms → 2000ms → 4000ms。这样避免了连续重试轰炸服务端，给它喘息的时间。

随机抖动（±25%）：想象一个场景——API 服务端过载返回 429，你的 Agent 等 1 秒后重试。问题是，全世界所有收到 429 的客户端都在等 1 秒后重试。1 秒后，服务端被又一波请求冲击——更多的 429，更多的等 1 秒，形成一个越来越大的请求洪峰。这就是"惊群效应"（Thundering Herd）。

解法就是在退避的基础上加一个随机偏移。每个客户端等的时间不一样，请求就自然分散了。我们这里用的是 ±25% 的 Equal Jitter——说白了就是在算出来的退避时间上下浮动 25%（比如 1 秒就随机取 0.75~1.25 秒），每个客户端等的时间略有不同，自然就错开了。简单够用，延迟不会太极端。关于不同 Jitter 策略的对比，AWS 有篇经典博客 Exponential Backoff And Jitter 讲得很透，感兴趣可以读一读。

接进 Agent Loop

接下来把重试逻辑接进 Agent Loop。关键变化就两个：用步骤级 try-catch 包裹整个 stream 消费过程，另外 maxRetries: 0 禁用 AI SDK 的内置重试，由我们全权接管。

src/agent-loop.ts
应用
复制
import { streamText, type ModelMessage } from 'ai';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';
import { isRetryable, calculateDelay, sleep } from './retry.js';

const MAX_STEPS = 15;
const MAX_RETRIES = 3;

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
) {
  let step = 0;
  resetHistory();

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    let hasToolCall = false;
    let fullText = '';
    let shouldBreak = false;
    let lastToolCall: { name: string; input: unknown } | null = null;
    let stepResponse: Awaited<ReturnType<typeof streamText>['response']>;

    // 步骤级重试：包裹整个 stream 消费过程
    for (let attempt = 1; ; attempt++) {
      try {
        const result = streamText({ model, system, tools, messages, maxRetries: 0, onError: () => {} });

        for await (const part of result.fullStream) {
          switch (part.type) {
            case 'text-delta':
              process.stdout.write(part.text);
              fullText += part.text;
              break;

            case 'tool-call': {
              hasToolCall = true;
              lastToolCall = { name: part.toolName, input: part.input };
              console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);

              const detection = detect(part.toolName, part.input);
              if (detection.stuck) {
                console.log(`  ${detection.message}`);
                if (detection.level === 'critical') {
                  shouldBreak = true;
                } else {
                  messages.push({
                    role: 'user' as const,
                    content: `[系统提醒] ${detection.message}。请换一个思路解决问题，不要重复同样的操作。`,
                  });
                }
              }
              recordCall(part.toolName, part.input);
              break;
            }

            case 'tool-result':
              console.log(`  [结果: ${JSON.stringify(part.output)}]`);
              if (lastToolCall) {
                recordResult(lastToolCall.name, lastToolCall.input, part.output);
              }
              break;
          }
        }

        stepResponse = await result.response;
        break;
      } catch (error) {
        if (attempt > MAX_RETRIES || !isRetryable(error as Error)) throw error;
        const delay = calculateDelay(attempt);
        console.log(`  [重试] 第 ${attempt}/${MAX_RETRIES} 次失败，${delay}ms 后重试...`);
        await sleep(delay);
        hasToolCall = false;
        fullText = '';
        shouldBreak = false;
        lastToolCall = null;
      }
    }

    if (shouldBreak) {
      console.log('\n[循环检测触发，Agent 已停止]');
      break;
    }

    messages.push(...stepResponse!.messages);

    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    console.log('  → 继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}


跑起来，输入「测试重试」：

bash
运行
复制
pnpm start

You: 测试重试

--- Step 1 ---
  [重试] 第 1/3 次失败，500ms 后重试...
  [重试] 第 2/3 次失败，1000ms 后重试...
重试成功！经过几次 429 错误后，我终于回来了。


看，mock model 连续抛了两次 429，我们的重试逻辑每次都等了一小段时间（500ms、1000ms），第三次就成功了。整个过程用户只看到几行重试提示，不会看到一堆 Error 堆栈——因为 maxRetries: 0 禁用了 SDK 内置重试，所有错误处理都在我们手里。

第三层：Token 预算

最后一层防护。你想想，前两层解决了"模型犯蠢"和"API 抽风"的问题，但还有一个隐患：Agent 每多跑一步，上下文就多一截，token 消耗是累积的。一个长对话跑下来，你可能都没意识到已经烧了多少钱。

这里先做最小可用版本——把每步的 token 用量累加起来，超了就停。更精细的预算管理（输入/输出分开计费、cache 命中折扣）后续章节再补齐。

更新 agent-loop.ts，把 budget 接入主循环：

src/agent-loop.ts
应用
复制
import { streamText, type ModelMessage } from 'ai';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';
import { isRetryable, calculateDelay, sleep } from './retry.js';

const MAX_STEPS = 15;
const MAX_RETRIES = 3;

export interface BudgetState {
  used: number;
  limit: number;
}

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
  budget: BudgetState,
) {
  let step = 0;
  resetHistory();

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    let hasToolCall = false;
    let fullText = '';
    let shouldBreak = false;
    let lastToolCall: { name: string; input: unknown } | null = null;
    let stepResponse: Awaited<ReturnType<typeof streamText>['response']>;
    let stepUsage: Awaited<ReturnType<typeof streamText>['usage']>;

    // 步骤级重试：包裹整个 stream 消费过程
    for (let attempt = 1; ; attempt++) {
      try {
        const result = streamText({ model, system, tools, messages, maxRetries: 0, onError: () => {} });

        for await (const part of result.fullStream) {
          switch (part.type) {
            case 'text-delta':
              process.stdout.write(part.text);
              fullText += part.text;
              break;

            case 'tool-call': {
              hasToolCall = true;
              lastToolCall = { name: part.toolName, input: part.input };
              console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);

              const detection = detect(part.toolName, part.input);
              if (detection.stuck) {
                console.log(`  ${detection.message}`);
                if (detection.level === 'critical') {
                  shouldBreak = true;
                } else {
                  messages.push({
                    role: 'user' as const,
                    content: `[系统提醒] ${detection.message}。请换一个思路解决问题，不要重复同样的操作。`,
                  });
                }
              }
              recordCall(part.toolName, part.input);
              break;
            }

            case 'tool-result':
              console.log(`  [结果: ${JSON.stringify(part.output)}]`);
              if (lastToolCall) {
                recordResult(lastToolCall.name, lastToolCall.input, part.output);
              }
              break;
          }
        }

        stepResponse = await result.response;
        stepUsage = await result.usage;
        break;
      } catch (error) {
        if (attempt > MAX_RETRIES || !isRetryable(error as Error)) throw error;
        const delay = calculateDelay(attempt);
        console.log(`  [重试] 第 ${attempt}/${MAX_RETRIES} 次失败，${delay}ms 后重试...`);
        await sleep(delay);
        hasToolCall = false;
        fullText = '';
        shouldBreak = false;
        lastToolCall = null;
      }
    }

    if (shouldBreak) {
      console.log('\n[循环检测触发，Agent 已停止]');
      break;
    }

    messages.push(...stepResponse!.messages);

    // Token 预算追踪：budget 由调用方持有，跨轮持续累计
    const inp = typeof stepUsage?.inputTokens === 'number' ? stepUsage.inputTokens : (stepUsage?.inputTokens?.total ?? 0);
    const out = typeof stepUsage?.outputTokens === 'number' ? stepUsage.outputTokens : (stepUsage?.outputTokens?.total ?? 0);
    budget.used += inp + out;
    const pct = Math.round(budget.used / budget.limit * 100);
    console.log(`  [Token] ${budget.used}/${budget.limit} (${pct}%)`);
    if (budget.used > budget.limit) {
      console.log('\n[Token 预算耗尽，强制停止]');
      break;
    }

    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    console.log('  → 继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}


跑起来，输入「测试预算」连续问 3-4 轮，每一步都会打印 Token 用量：

bash
运行
复制
pnpm start


Mock model 在「测试预算」模式下每步模拟消耗 4500 tokens（输入 3000 + 输出 1500），limit 设为 15000。第 3 次问的时候累计 13500/15000，已经飙到 90%；第 4 次再加 4500 就超过预算，触发熔断。普通对话和工具调用每步只消耗几百 tokens，不会被预算抢戏——这样三层防护可以独立演示，互不干扰。

实际真实 API 调用时，usage 返回的是真实消耗，输入 token 会随上下文累积越来越大，单步可能就几千甚至上万。预算根据场景调——简单问答 Agent 50000 起步，Coding Agent 动辄几十万 tokens，要更大的预算。

到这里，你的 Agent Loop 已经有了三层独立的防护，而且都是非侵入式的——Agent Loop 的核心逻辑没变，防护只在边界上做检查。后面要调阈值或换检测策略，改对应文件就行，不用动主循环。

防护到位了，但 Agent 的能力还很有限——就两个玩具工具。接下来的章节我们来做个正经的工具系统：注册机制、执行管线、结果截断、并发控制，给 Agent 真正有用的手脚。我们下一节，再见。

参考链接
Exponential Backoff And Jitter (AWS Architecture Blog)
Vercel AI SDK - Error Handling
Retries, Fallbacks, and Circuit Breakers in LLM Apps
上一篇
从"能聊天"到"能干活"——给 Agent 装上 while 循环
下一篇 · 第二章：Tool System
给 Agent 一双手——Tool 注册、执行、截断与并发
编辑器


---
## 代码块


```
--- Step 1 ---
  [调用: read_file({"path":"/tmp/data.csv"})]
  [结果: "文件不存在"]

--- Step 2 ---
  [调用: read_file({"path":"/tmp/data.csv"})]
  [结果: "文件不存在"]

--- Step 3 ---
  [调用: read_file({"path":"/tmp/data.csv"})]
  [结果: "文件不存在"]

...（一直到 Step 10，MAX_STEPS 救了你一命）
```


```bash
pnpm install
```


```typescript
import 'dotenv/config';
import { type ModelMessage } from 'ai';
import { createOpenAI } from '@ai-sdk/openai';
import { createMockModel } from './mock-model.js';
import { createInterface } from 'node:readline';
import { weatherTool, calculatorTool } from './tools.js';
import { agentLoop, type BudgetState } from './agent-loop.js';

const qwen = createOpenAI({
  baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
  apiKey: process.env.DASHSCOPE_API_KEY,
});

const model = process.env.DASHSCOPE_API_KEY
  ? qwen.chat('qwen-plus-latest')
  : createMockModel();

const tools = { get_weather: weatherTool, calculator: calculatorTool };
const messages: ModelMessage[] = [];
// 预算由调用方持有，跨轮持续累计——agentLoop 只负责消费它
const budget: BudgetState = { used: 0, limit: 15000 };
const rl = createInterface({ input: process.stdin, output: process.stdout });

const SYSTEM = `你是 Super Agent，一个有工具调用能力的 AI 助手。
需要查询信息时，主动使用工具，不要编造数据。
回答要简洁直接。`;

function ask() {
  rl.question('\nYou: ', async (input) => {
    const trimmed = input.trim();
    if (!trimmed || trimmed === 'exit') {
      console.log('Bye!');
      rl.close();
      return;
    }

    messages.push({ role: 'user', content: trimmed });

    await agentLoop(model, tools, messages, SYSTEM, budget);

    ask();
  });
}

console.log('Super Agent v0.3 — Fuses (type "exit" to quit)\n');
console.log('试试输入："测试死循环"、"测试重试"、"测试预算" 看三层防护效果\n');
ask();
```


```typescript
// ... mock model 核心结构同上一篇 ...
// 新增：死循环模拟 + API 错误模拟

// 在 detectToolIntent 中新增：
// 1. 用户说"测试死循环" → 每次都返回 get_weather({"city":"北京"})
// 2. 用户说"测试重试" → 抛出 429 错误
```


```typescript
import { createHash } from 'node:crypto';

// --- 类型定义 ---

export interface ToolCallRecord {
  toolName: string;
  argsHash: string;
  resultHash?: string;
  timestamp: number;
}

export type DetectorKind = 'generic_repeat' | 'ping_pong' | 'global_circuit_breaker';

export type DetectionResult =
  | { stuck: false }
  | { stuck: true; level: 'warning' | 'critical'; detector: DetectorKind; count: number; message: string };

// --- 配置 ---

const HISTORY_SIZE = 30;       // 滑动窗口大小
const WARNING_THRESHOLD = 5;   // 警告阈值（演示用，生产环境通常是 10）
const CRITICAL_THRESHOLD = 8;  // 严重阈值（演示用，生产环境通常是 20）
const BREAKER_THRESHOLD = 10;  // 熔断阈值（演示用，生产环境通常是 30）

// --- 指纹计算 ---

function stableStringify(value: unknown): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(',')}]`;
  const keys = Object.keys(value as Record<string, unknown>).sort();
  return `{${keys.map(k => `${JSON.stringify(k)}:${stableStringify((value as any)[k])}`).join(',')}}`;
}

function hash(input: string): string {
  return createHash('sha256').update(input).digest('hex').slice(0, 16);
}

export function hashToolCall(toolName: string, params: unknown): string {
  return `${toolName}:${hash(stableStringify(params))}`;
}

export function hashResult(result: unknown): string {
  return hash(stableStringify(result));
}

// --- 滑动窗口 ---

const history: ToolCallRecord[] = [];

export function recordCall(toolName: string, params: unknown): void {
  history.push({
    toolName,
    argsHash: hashToolCall(toolName, params),
    timestamp: Date.now(),
  });
  if (history.length > HISTORY_SIZE) history.shift();
}

export function recordResult(toolName: string, params: unknown, result: unknown): void {
  const argsHash = hashToolCall(toolName, params);
  const resultH = hashResult(result);
  for (let i = history.length - 1; i >= 0; i--) {
    if (history[i].toolName === toolName && history[i].argsHash === argsHash && !history[i].resultHash) {
      history[i].resultHash = resultH;
      break;
    }
  }
}

export function resetHistory(): void {
  history.length = 0;
}

// --- 检测器 ---

function getNoProgressStreak(toolName: string, argsHash: string): number {
  let streak = 0;
  let lastResultHash: string | undefined;
  for (let i = history.length - 1; i >= 0; i--) {
    const r = history[i];
    if (r.toolName !== toolName || r.argsHash !== argsHash) continue;
    if (!r.resultHash) continue;
    if (!lastResultHash) { lastResultHash = r.resultHash; streak = 1; continue; }
    if (r.resultHash !== lastResultHash) break;
    streak++;
  }
  return streak;
}

function getPingPongCount(currentHash: string): number {
  if (history.length < 3) return 0;
  const last = history[history.length - 1];
  let otherHash: string | undefined;
  for (let i = history.length - 2; i >= 0; i--) {
    if (history[i].argsHash !== last.argsHash) { otherHash = history[i].argsHash; break; }
  }
  if (!otherHash) return 0;
  let count = 0;
  for (let i = history.length - 1; i >= 0; i--) {
    const expected = count % 2 === 0 ? last.argsHash : otherHash;
    if (history[i].argsHash !== expected) break;
    count++;
  }
  if (currentHash === otherHash && count >= 2) return count + 1;
  return 0;
}

// --- 主检测函数 ---

export function detect(toolName: string, params: unknown): DetectionResult {
  const argsHash = hashToolCall(toolName, params);
  const noProgress = getNoProgressStreak(toolName, argsHash);

  if (noProgress >= BREAKER_THRESHOLD) {
    return { stuck: true, level: 'critical', detector: 'global_circuit_breaker', count: noProgress,
      message: `[熔断] ${toolName} 已重复 ${noProgress} 次且无进展，强制停止` };
  }

  const pingPong = getPingPongCount(argsHash);
  if (pingPong >= CRITICAL_THRESHOLD) {
    return { stuck: true, level: 'critical', detector: 'ping_pong', count: pingPong,
      message: `[熔断] 检测到乒乓循环（${pingPong} 次交替），强制停止` };
  }
  if (pingPong >= WARNING_THRESHOLD) {
    return { stuck: true, level: 'warning', detector: 'ping_pong', count: pingPong,
      message: `[警告] 检测到乒乓循环（${pingPong} 次交替），建议换个思路` };
  }

  const recentCount = history.filter(h => h.toolName === toolName && h.argsHash === argsHash).length;
  if (recentCount >= CRITICAL_THRESHOLD) {
    return { stuck: true, level: 'critical', detector: 'generic_repeat', count: recentCount,
      message: `[熔断] ${toolName} 相同参数已调用 ${recentCount} 次，强制停止` };
  }
  if (recentCount >= WARNING_THRESHOLD) {
    return { stuck: true, level: 'warning', detector: 'generic_repeat', count: recentCount,
      message: `[警告] ${toolName} 相同参数已调用 ${recentCount} 次，你可能陷入了重复` };
  }

  return { stuck: false };
}
```


```typescript
import { streamText, type ModelMessage } from 'ai';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';

const MAX_STEPS = 15;

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
) {
  let step = 0;
  resetHistory();

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    const result = await streamText({ model, system, tools, messages, maxRetries: 0, onError: () => {} });

    let hasToolCall = false;
    let fullText = '';
    let shouldBreak = false;
    let lastToolCall: { name: string; input: unknown } | null = null;

    for await (const part of result.fullStream) {
      switch (part.type) {
        case 'text-delta':
          process.stdout.write(part.text);
          fullText += part.text;
          break;

        case 'tool-call': {
          hasToolCall = true;
          lastToolCall = { name: part.toolName, input: part.input };
          console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);

          const detection = detect(part.toolName, part.input);
          if (detection.stuck) {
            console.log(`  ${detection.message}`);
            if (detection.level === 'critical') {
              shouldBreak = true;
            } else {
              messages.push({
                role: 'user' as const,
                content: `[系统提醒] ${detection.message}。请换一个思路解决问题，不要重复同样的操作。`,
              });
            }
          }
          recordCall(part.toolName, part.input);
          break;
        }

        case 'tool-result':
          console.log(`  [结果: ${JSON.stringify(part.output)}]`);
          if (lastToolCall) {
            recordResult(lastToolCall.name, lastToolCall.input, part.output);
          }
          break;
      }
    }

    if (shouldBreak) {
      console.log('\n[循环检测触发，Agent 已停止]');
      break;
    }

    const stepResult = await result.response;
    messages.push(...stepResult.messages);

    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    console.log('  → 继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}
```


```bash
pnpm start
```


```
You: 测试死循环

--- Step 1 ---
  [调用: get_weather({"city":"北京"})]
  [结果: "晴，15-25°C，东南风 2 级"]
  → 继续下一步...

--- Step 2 ~ 5 ---
  （同样的调用重复 4 次...）

--- Step 6 ---
  [调用: get_weather({"city":"北京"})]
  [警告] get_weather 相同参数已调用 5 次，你可能陷入了重复
  [结果: "晴，15-25°C，东南风 2 级"]
  → 继续下一步...

--- Step 7 ---
根据查询结果：晴，15-25°C，东南风 2 级
```


```typescript
// --- 错误分类 ---

export function isRetryable(error: unknown): boolean {
  if (!(error instanceof Error)) return false;

  const message = error.message || '';

  // HTTP 状态码判断
  const statusMatch = message.match(/(\d{3})/);
  if (statusMatch) {
    const status = parseInt(statusMatch[1]);
    if ([429, 529, 408].includes(status)) return true;
    if (status >= 500 && status < 600) return true;
    if (status >= 400 && status < 500) return false;
  }

  // 网络错误
  if (message.includes('ECONNRESET') || message.includes('EPIPE')) return true;
  if (message.includes('ETIMEDOUT') || message.includes('timeout')) return true;
  if (message.includes('fetch failed') || message.includes('network')) return true;
  // AI SDK 会把流式错误包装成 NoOutputGeneratedError
  if (message.includes('No output generated')) return true;

  return false;
}

// --- 指数退避 + 随机抖动 ---

export function calculateDelay(attempt: number, baseMs = 500, maxMs = 30000): number {
  const exponential = baseMs * Math.pow(2, attempt - 1);
  const capped = Math.min(exponential, maxMs);
  const jitterRange = capped * 0.25;
  const jittered = capped + (Math.random() * 2 - 1) * jitterRange;
  return Math.max(0, Math.round(jittered));
}

export function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}
```


```typescript
import { streamText, type ModelMessage } from 'ai';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';
import { isRetryable, calculateDelay, sleep } from './retry.js';

const MAX_STEPS = 15;
const MAX_RETRIES = 3;

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
) {
  let step = 0;
  resetHistory();

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    let hasToolCall = false;
    let fullText = '';
    let shouldBreak = false;
    let lastToolCall: { name: string; input: unknown } | null = null;
    let stepResponse: Awaited<ReturnType<typeof streamText>['response']>;

    // 步骤级重试：包裹整个 stream 消费过程
    for (let attempt = 1; ; attempt++) {
      try {
        const result = streamText({ model, system, tools, messages, maxRetries: 0, onError: () => {} });

        for await (const part of result.fullStream) {
          switch (part.type) {
            case 'text-delta':
              process.stdout.write(part.text);
              fullText += part.text;
              break;

            case 'tool-call': {
              hasToolCall = true;
              lastToolCall = { name: part.toolName, input: part.input };
              console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);

              const detection = detect(part.toolName, part.input);
              if (detection.stuck) {
                console.log(`  ${detection.message}`);
                if (detection.level === 'critical') {
                  shouldBreak = true;
                } else {
                  messages.push({
                    role: 'user' as const,
                    content: `[系统提醒] ${detection.message}。请换一个思路解决问题，不要重复同样的操作。`,
                  });
                }
              }
              recordCall(part.toolName, part.input);
              break;
            }

            case 'tool-result':
              console.log(`  [结果: ${JSON.stringify(part.output)}]`);
              if (lastToolCall) {
                recordResult(lastToolCall.name, lastToolCall.input, part.output);
              }
              break;
          }
        }

        stepResponse = await result.response;
        break;
      } catch (error) {
        if (attempt > MAX_RETRIES || !isRetryable(error as Error)) throw error;
        const delay = calculateDelay(attempt);
        console.log(`  [重试] 第 ${attempt}/${MAX_RETRIES} 次失败，${delay}ms 后重试...`);
        await sleep(delay);
        hasToolCall = false;
        fullText = '';
        shouldBreak = false;
        lastToolCall = null;
      }
    }

    if (shouldBreak) {
      console.log('\n[循环检测触发，Agent 已停止]');
      break;
    }

    messages.push(...stepResponse!.messages);

    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    console.log('  → 继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}
```


```bash
pnpm start
```


```
You: 测试重试

--- Step 1 ---
  [重试] 第 1/3 次失败，500ms 后重试...
  [重试] 第 2/3 次失败，1000ms 后重试...
重试成功！经过几次 429 错误后，我终于回来了。
```


```typescript
import { streamText, type ModelMessage } from 'ai';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';
import { isRetryable, calculateDelay, sleep } from './retry.js';

const MAX_STEPS = 15;
const MAX_RETRIES = 3;

export interface BudgetState {
  used: number;
  limit: number;
}

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
  budget: BudgetState,
) {
  let step = 0;
  resetHistory();

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    let hasToolCall = false;
    let fullText = '';
    let shouldBreak = false;
    let lastToolCall: { name: string; input: unknown } | null = null;
    let stepResponse: Awaited<ReturnType<typeof streamText>['response']>;
    let stepUsage: Awaited<ReturnType<typeof streamText>['usage']>;

    // 步骤级重试：包裹整个 stream 消费过程
    for (let attempt = 1; ; attempt++) {
      try {
        const result = streamText({ model, system, tools, messages, maxRetries: 0, onError: () => {} });

        for await (const part of result.fullStream) {
          switch (part.type) {
            case 'text-delta':
              process.stdout.write(part.text);
              fullText += part.text;
              break;

            case 'tool-call': {
              hasToolCall = true;
              lastToolCall = { name: part.toolName, input: part.input };
              console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);

              const detection = detect(part.toolName, part.input);
              if (detection.stuck) {
                console.log(`  ${detection.message}`);
                if (detection.level === 'critical') {
                  shouldBreak = true;
                } else {
                  messages.push({
                    role: 'user' as const,
                    content: `[系统提醒] ${detection.message}。请换一个思路解决问题，不要重复同样的操作。`,
                  });
                }
              }
              recordCall(part.toolName, part.input);
              break;
            }

            case 'tool-result':
              console.log(`  [结果: ${JSON.stringify(part.output)}]`);
              if (lastToolCall) {
                recordResult(lastToolCall.name, lastToolCall.input, part.output);
              }
              break;
          }
        }

        stepResponse = await result.response;
        stepUsage = await result.usage;
        break;
      } catch (error) {
        if (attempt > MAX_RETRIES || !isRetryable(error as Error)) throw error;
        const delay = calculateDelay(attempt);
        console.log(`  [重试] 第 ${attempt}/${MAX_RETRIES} 次失败，${delay}ms 后重试...`);
        await sleep(delay);
        hasToolCall = false;
        fullText = '';
        shouldBreak = false;
        lastToolCall = null;
      }
    }

    if (shouldBreak) {
      console.log('\n[循环检测触发，Agent 已停止]');
      break;
    }

    messages.push(...stepResponse!.messages);

    // Token 预算追踪：budget 由调用方持有，跨轮持续累计
    const inp = typeof stepUsage?.inputTokens === 'number' ? stepUsage.inputTokens : (stepUsage?.inputTokens?.total ?? 0);
    const out = typeof stepUsage?.outputTokens === 'number' ? stepUsage.outputTokens : (stepUsage?.outputTokens?.total ?? 0);
    budget.used += inp + out;
    const pct = Math.round(budget.used / budget.limit * 100);
    console.log(`  [Token] ${budget.used}/${budget.limit} (${pct}%)`);
    if (budget.used > budget.limit) {
      console.log('\n[Token 预算耗尽，强制停止]');
      break;
    }

    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    console.log('  → 继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}
```


```bash
pnpm start
```
