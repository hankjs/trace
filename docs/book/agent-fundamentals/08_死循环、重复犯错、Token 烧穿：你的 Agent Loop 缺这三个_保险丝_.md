# 死循环、重复犯错、Token 烧穿：你的 Agent Loop 缺这三个"保险丝"

课程
吃透 AI Agent 开发
死循环、重复犯错、Token 烧穿：你的 Agent Loop 缺这三个"保险丝"
死循环、重复犯错、Token 烧穿：你的 Agent Loop 缺这三个"保险丝"

约 48 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
回到私教
重新学习

假设你给 Agent 下了一个指令：「把这个项目里所有的 console.log 替换成 logger.info」。

Agent 开始干活了：读文件、找 console.log、替换、写回文件。看起来一切正常。

但你没注意到，它改完一个文件之后又读了一遍，发现「还有 console.log」（因为 logger.info 这个字符串里恰好也包含 log），于是又改了一遍，然后又读了一遍，又改了一遍……

15 分钟后你回来一看：跑了 200 轮，烧了 $50 的 Token，那个文件已经被改得面目全非了。

这不是假设。在生产环境里，Agent 出问题最常见的三种方式就是：

死循环：反复调用同一个工具，做同样的事
Token 烧穿：无限续写，上下文越来越大，钱越烧越多
输出截断：模型话说到一半被截了，自己还不知道

上一篇我们讲了 API 挂了怎么办。这篇讲一个更隐蔽的问题——API 没挂，但 Agent 自己作死了。对 Agent 工程而言，这是必须要警惕的问题，接下来就让我们展开拆解这类问题。

保险丝 1：工具死循环检测

先说死循环。这是 Agent 开发中最常见也最危险的问题。

为什么危险？因为它看起来在正常工作。日志里一堆成功的工具调用，每一次都返回了正常结果。你不仔细看，根本发现不了它在原地踏步。

OpenClaw 在这个问题上下了很大功夫，设计了四种检测器来抓不同类型的死循环。

核心思路：给每次调用"打指纹"

怎么判断一个工具调用是不是重复的？

最直观的想法：比较工具名 + 参数是否一样。但参数可能是个很大的对象（比如一整个文件内容），逐字比较太慢。

OpenClaw 的做法是哈希指纹：把工具名和参数丢进 SHA256，算出一个固定长度的哈希值。两次调用的哈希值一样，就认为是同样的调用。

指纹 = SHA256(工具名 + 稳定序列化(参数))


这里有个细节：稳定序列化。JavaScript 对象的 key 顺序是不确定的，{a: 1, b: 2} 和 {b: 2, a: 1} 序列化出来可能不一样。所以在哈希之前，要先把 key 排序，确保同样的参数一定产出同样的哈希值。

但光看参数相同还不够。同样的参数，结果可能不一样。 比如你读同一个文件 10 次，但每次读到的内容都不一样（因为有其他进程在修改），这不算死循环——每次都有新信息。

所以 OpenClaw 不光记录调用指纹，还记录结果指纹。只有「同样的调用 + 同样的结果」才被认定为无进展。

类比一个场景：你打电话给客服 10 次，每次得到的答复都是「正在处理中」——这是死循环。但如果每次得到不同的进展信息——这是正常跟进。

四种检测器

第一种：通用重复检测

最简单粗暴：同一个工具 + 同一个参数被调用了 N 次。

触发阈值是 10 次。但这个检测器只告警，不阻断。为什么不阻断？因为有些工具确实会被合法地反复调用。比如你让 Agent 处理 20 个文件，read_file 被用相同的参数调用多次可能只是因为 Agent 在不同的推理步骤需要重新读取。

第二种：无进展轮询检测

专门针对轮询类的工具。

什么是轮询？比如 Agent 启动了一个后台任务，然后不停地查状态，比如轮询部署状态、检查服务的健康检查——如果每次查到的状态都一样，这就是无进展的轮询。

轮询检测更多是一个防御性设计——你不一定会频繁遇到，但一旦遇到（比如模型误判某个任务没完成，反复查同一个状态），没有这根保险丝就会烧 token。

这个检测器比第一种更严格：不光看参数一样，还看结果一样。如果状态真的长时间没变，Agent 应该干点别的去。

第三种：Ping-Pong 检测

这是最巧妙的一种。检测两个工具交替调用的模式：

read_file → write_file → read_file → write_file → ...


开头那个 console.log 的例子就是典型的 Ping-Pong 循环。

检测算法是从最近的调用往回扫，看是否存在 A→B→A→B 的交替模式。关键判断条件是：两边的结果都没变化。

如果读文件每次内容不同（说明写入确实生效了），那不算——这是正常的读-改-读-改流程。只有读文件每次内容一样、写文件每次也是一样的内容，才说明 Agent 在原地打转。

第四种：全局熔断器

上面的三种情况，说白了就是无进展调用。

只要累计 30 次无进展调用，强制停止，没有例外。

这是最后一道防线。即便前三种检测器都被关了或者都没触发，全局熔断器永远在线。

这四种检测器在 OpenClaw 里面都有完整的实现，共同来确保生产环境工具调用的安全性。

三级响应：不是一上来就断

这四种检测器共享一套三级响应机制：

级别	阈值	行为
Warning	10 次	记日志，工具继续执行
Critical	20 次	阻断工具，Agent 收到错误
Break	30 次	全局熔断，强制停止

为什么不在第一次重复就停？

因为误杀的代价太大。你把一个正在正常工作的 Agent 强行停了，比让它多跑几轮浪费的时间和金钱更多。所以第一级只是告警——给 Agent 一个提醒，看它能不能自己调整策略。到了 20 次，基本可以确认是死循环了，这时候才动手阻断。

还有一个防刷屏的设计：告警不是每次都发的，而是每 10 次发一次。第 10 次发一个、第 20 次发一个，不会在 10 到 19 之间每次都发。

这里画了张图给你总结一下：

动手试试：死循环长什么样

光说不练假把式。右边的编辑器里已经准备好了一个 Mock Agent 环境——不需要 API Key，纯本地模拟。Mock LLM 会根据场景模拟不同的失控行为，也能"读懂"你注入的干预消息并改变策略。

先点「应用」把裸 Agent Loop 写入编辑器，然后点「运行」看看没有保险丝的 Agent 会发生什么：

src/agent.ts
复制
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { Message } from './types.js';

setScenario('dead_loop');

const messages: Message[] = [
  { role: 'user', content: '把所有 console.log 替换成 logger.info' }
];
const MAX_TURNS = 30;
let turn = 0;

console.log('--- 裸 Agent Loop（无保险丝）---\n');

while (true) {
  turn++;
  const res = await chat(messages);
  console.log(res.text);

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 正常完成');
    break;
  }

  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    console.log(`  🔧 ${tool.name}(${JSON.stringify(tool.params)})`);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });

  if (turn >= MAX_TURNS) {
    console.log(`\n💀 跑满 ${MAX_TURNS} 轮还没停！这就是死循环。`);
    console.log('没有保险丝 → 无限循环 → 烧钱 + 用户卡死');
    break; // demo 安全阀，真实环境没有这个
  }
}

bash
复制
npx tsx src/agent.ts


30 轮跑满，read → write → read → write，经典 Ping-Pong 死循环。

现在点「应用」加上死循环检测。注意看关键变化：检测到重复后不是直接停，而是先注入干预消息让模型换策略——就像真实生产环境一样：

src/agent.ts
复制
import { createHash } from 'node:crypto';
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { ToolCall, Message } from './types.js';

// ———— 保险丝 1：死循环检测 ————

/** 把工具名+参数哈希成指纹，用于判断是否重复 */
function fingerprint(name: string, params: unknown): string {
  const stable = JSON.stringify(params, Object.keys((params || {}) as any).sort());
  return createHash('sha256').update(name + stable).digest('hex').slice(0, 12);
}

/** 记录每个指纹的调用次数和上次结果 */
const history = new Map<string, { count: number; lastResult: string }>();

/** 检查一次工具调用是否构成死循环 */
function checkLoop(tool: ToolCall, result: string): 'ok' | 'warn' | 'break' {
  const fp = fingerprint(tool.name, tool.params);
  const entry = history.get(fp) || { count: 0, lastResult: '' };

  // 关键：同样的调用 + 同样的结果 = 无进展
  if (entry.lastResult === result) {
    entry.count++;
  } else {
    entry.count = 1; // 结果变了，重新计数
  }
  entry.lastResult = result;
  history.set(fp, entry);

  if (entry.count >= 10) return 'break'; // 硬停
  if (entry.count >= 5) return 'warn';   // 软干预
  return 'ok';
}

// ———— Agent Loop ————
setScenario('dead_loop');
const messages: Message[] = [
  { role: 'user', content: '把所有 console.log 替换成 logger.info' }
];
let turn = 0;
let warnInjected = false;

console.log('--- Agent Loop + 死循环检测 ---\n');

while (true) {
  turn++;
  const res = await chat(messages);
  console.log(`[轮次 ${turn}] ${res.text}`);

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 干预生效，Agent 换了策略并完成任务');
    break;
  }

  let stopped = false;
  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    const status = checkLoop(tool, result);

    if (status === 'break') {
      console.log(`\n⛔ 强制停止：${tool.name} 重复 10 次且结果相同`);
      console.log('💾 已执行的工具结果已保留');
      console.log('💡 建议：拆分任务或手动检查');
      stopped = true;
      break;
    }

    if (status === 'warn' && !warnInjected) {
      console.log('  ⚠️ 检测到重复！注入干预消息...');
      messages.push({
        role: 'system',
        content: '[LOOP_WARNING] 你正在反复用相同方式操作且没有进展。请换一种方式完成任务，比如用 bash + sed 批量替换。'
      });
      warnInjected = true;
    }

    console.log(`  🔧 ${tool.name} → ${result.slice(0, 50)}`);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
  if (stopped) break;
}

bash
复制
npx tsx src/agent.ts


看输出：前几轮还在 ping-pong，检测到重复后注入干预消息，模型收到后切换到 bash + sed 批量替换，任务完成。这才是生产级的处理——先干预，干预无效再强制停止。

保险丝 2：Token 预算控制

死循环检测管的是工具层面的重复。但还有一种作死方式不涉及重复工具调用——模型无限续写。

你有没有遇到过这种情况：让 Agent 生成一个文档，它写了 2000 字还没停，越写越长，把整个上下文都快塞满了。这个偶然会发生，我自己也多次遇到过这种问题，如果你要做 Agent，这种问题不得不专门来防范。

Claude Code 有一个 Token 预算系统来应对这个问题。

怎么工作的

核心逻辑：给 Agent 设一个输出 Token 预算。比如这次任务最多允许输出 30000 个 Token。系统会跟踪累计的输出 Token 数，然后做两件事：

第一件：90% 时注入 nudge 消息

当输出量达到预算的 90% 时，系统往消息流里注入一条提醒：

"已完成 Token 目标的 87%（26,100 / 30,000）。继续工作——不要总结。"

为什么是 90% 而不是 100%？因为模型不能精确控制自己的输出长度。如果在 100% 的时候才提醒，可能已经超了。提前 10% 提醒，给模型一个缓冲区来收尾。

为什么说「不要总结」？因为模型收到「快到限制了」的信号后，本能反应就是开始总结、收尾。模型也会"慌"。

但如果任务还没完成，总结反而浪费 Token。明确告诉它「继续干活，别总结」，能更有效地利用剩余预算。

第二件：检测递减回报

如果 Agent 已经续写了 3 次以上，而且最近两次每次只增加了不到 500 个 Token——系统判定为递减回报，直接停止。

什么意思呢？就是 Agent 可能陷入了一种「每次只说一点点新东西」的模式。可能在反复润色同一段文字，或者在添加一些无关紧要的细节。每轮增量不到 500 Token，说明没有实质性进展了。

续写第1次：+3000 Token   ✓ 正常
续写第2次：+2500 Token   ✓ 正常
续写第3次：+400 Token    ⚠️ 增量很小
续写第4次：+300 Token    ⛔ 连续两次递减，停止

经济账

算一笔账。假设你用的是 Claude Sonnet，输出价格大约 $15 / 百万 Token。

一次失控的 Agent 跑 200 轮，假设每轮输出 1000 Token：

200 轮 × 1000 Token = 200,000 Token
200,000 × $15 / 1,000,000 = $3


$3 看起来不多，但别忘了输入 Token 才是大头。每轮 Agent 调用都要带上完整的上下文——System Prompt + 对话历史 + 工具定义。200 轮下来，累计输入 Token 可能是输出的 10-20 倍。

一次失控，$50-100 真的不是开玩笑。

动手试试：Token 烧穿

场景切到 token_burn——模型每轮输出 3000 tokens 的"详细分析"，不给它刹车就会一直烧。关键看：90% 预算时注入 nudge 消息，模型收到后主动精简收尾：

src/agent.ts
复制
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { Message } from './types.js';

// ———— 保险丝 2：Token 预算 ————

const TOKEN_BUDGET = 15000;
let totalOutput = 0;
let lowStreak = 0;

function checkBudget(tokens: number): 'ok' | 'nudge' | 'stop' {
  totalOutput += tokens;

  // 递减回报：只在已经有实质输出后才检查（避免小输出场景误触发）
  if (totalOutput > 5000) {
    if (tokens < 500) {
      lowStreak++;
    } else {
      lowStreak = 0;
    }
    if (lowStreak >= 2) return 'stop';
  }

  // 90% 预算 → 注入 nudge
  if (totalOutput >= TOKEN_BUDGET * 0.9) return 'nudge';
  return 'ok';
}

// ———— Agent Loop ————

setScenario('token_burn');
const messages: Message[] = [
  { role: 'user', content: '分析这个项目的所有模块，给出完整报告' }
];
let turn = 0;
let nudgeInjected = false;

console.log('--- 只有保险丝 2：Token 预算 ---');
console.log(`预算上限: ${TOKEN_BUDGET} tokens\n`);

while (true) {
  turn++;
  const res = await chat(messages);

  const status = checkBudget(res.outputTokens);
  console.log(`[轮次 ${turn}] +${res.outputTokens} tokens（累计 ${totalOutput}/${TOKEN_BUDGET}）`);

  if (status === 'stop') {
    console.log('\n⛔ 连续递减回报，强制停止');
    console.log(`💾 已分析 ${turn} 个模块的结果已保留`);
    break;
  }

  if (status === 'nudge' && !nudgeInjected) {
    console.log(`  📢 90% 预算已用！注入 nudge...`);
    messages.push({
      role: 'system',
      content: `[BUDGET_NUDGE] Token 预算已用 ${Math.round(totalOutput / TOKEN_BUDGET * 100)}%。请精简输出，给关键结论。继续工作——不要总结前面做过的事。`
    });
    nudgeInjected = true;
  }

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 收到 nudge 后精简收尾');
    console.log(`📊 输出: ${res.text.slice(0, 80)}...`);
    break;
  }

  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    console.log(`  🔧 ${tool.name}(${(tool.params as any).module || ''}) → ${result.slice(0, 40)}`);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
}

console.log(`\n总计输出: ${totalOutput} tokens`);

bash
复制
npx tsx src/agent.ts


注意看：前几轮每轮 3000 tokens，90% 预算时注入 nudge，模型收到后只输出 80 tokens 的精简结论就收尾。nudge 措辞很关键——"继续工作不要总结"，防止模型收到预算提醒后本能地回顾总结，浪费剩余预算。

保险丝 3：输出截断恢复

第三种作死方式更隐蔽：模型的话说到一半被截了。

每个模型都有一个 max_output_tokens 限制。Claude 默认是 8192 个 Token。如果模型要说的话超过了这个限制，输出会被硬截断。

问题是：模型自己不知道被截了。 它以为自己说完了（因为生成确实停止了），但实际上后面还有半句话没说出来。

如果这个截断发生在一个工具调用的 JSON 中间呢？JSON 不完整，解析直接失败，Agent 不知道该干什么。

Claude Code 的处理方式比较通用，分为三步递进恢复：

第一步：提高上限

默认 8K 不够？那试试 64K。

这是最简单的办法——很多时候模型只是碰巧输出多了一点，把上限提高就行。这一步是静默重试，用户完全无感。

第二步：注入恢复消息

如果 64K 也不够（或者提高之后还是被截了），往消息流里注入一条指令：

"输出 Token 限制被触发了。直接从断点继续——不要道歉，不要回顾你在做什么。如果是说到一半被截了，从那个思路接着说。把剩下的工作拆成更小的块。"

这条消息设计得很精准：

「不要道歉」——模型的第一反应是「抱歉，我的回复被截断了」，这浪费 Token
「不要回顾」——模型的第二反应是把前面说的复述一遍，也浪费 Token
「从断点继续」——直接接上
「拆成更小的块」——防止下一次又被截断

这一步最多执行 3 次。

第三步：认栽

3 次恢复都不行？那就把不完整的结果返回给用户，标记为输出被截断。

为什么不无限重试？因为如果模型在 64K 的限制下连续 3 次都说不完，说明要么任务拆分有问题，要么模型在做一些不必要的展开。这时候人工介入比自动重试更有效。

动手试试：截断恢复

场景切到 truncation。这次只看保险丝 3——handleTruncation 往消息里注入恢复指令，第一次温和（"从断点继续"），第二次更强硬（"大幅精简"）。mock LLM 统计收到几条恢复消息：0 条→截断，1 条→再截断，≥2 条→精简完成：

src/agent.ts
复制
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { Message } from './types.js';

// ———— 保险丝 3：截断恢复 ————

const MAX_RECOVERY = 3;
let recoveryCount = 0;

function handleTruncation(messages: Message[]): 'retry' | 'give_up' {
  recoveryCount++;
  if (recoveryCount > MAX_RECOVERY) return 'give_up';

  // 第一次温和，后续更强硬
  const msg = recoveryCount === 1
    ? '直接从断点继续——不要道歉，不要回顾。把剩余工作拆成更小的块。'
    : '再次被截断。请大幅精简输出，只列关键结论。';

  console.log(`  🔄 恢复 ${recoveryCount}/${MAX_RECOVERY}: "${msg.slice(0, 25)}..."`);
  messages.push({
    role: 'system',
    content: `[TRUNCATION_RECOVERY] ${msg}`,
  });
  return 'retry';
}

// ———— Agent Loop ————

setScenario('truncation');
const messages: Message[] = [
  { role: 'user', content: '分析 package.json 的所有依赖，列出需要更新的' }
];
let turn = 0;

console.log('--- 只有保险丝 3：截断恢复 ---\n');

while (true) {
  turn++;
  const res = await chat(messages);

  if (res.stopReason === 'max_tokens') {
    console.log(`[轮次 ${turn}] ⚠️ 输出被截断 (max_tokens)`);
    const action = handleTruncation(messages);
    if (action === 'give_up') {
      console.log('\n⛔ 连续截断 3 次，放弃');
      console.log('💾 部分内容已保留，标记为"不完整"');
      break;
    }
    continue; // 带着恢复指令重试
  }

  console.log(`[轮次 ${turn}] ${res.text.slice(0, 50)}... (${res.outputTokens} tokens)`);

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 截断恢复成功！');
    console.log(`📊 输出:\n${res.text}`);
    break;
  }

  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
}

console.log(`\n截断恢复次数: ${recoveryCount}`);

bash
复制
npx tsx src/agent.ts


前两轮被截断，每次注入不同的恢复指令，第三轮模型精简输出后完成。

Agent 什么时候该停？七种退出路径

讲完三个保险丝，最后来看一个更大的问题：Agent 到底有哪些方式退出循环？

很多人只想到两种：任务完成了退出，或者出错了退出。但实际上，一个生产级的 Agent 至少有七种退出路径。

1. completed——正常完成

模型返回 end_turn，表示它认为任务做完了。这是最理想的退出方式。

2. max_turns——超过最大轮次

你给 Agent 设了一个上限，比如最多 20 轮。跑满了就强制停。

这个上限有两层含义：

防死循环：如果 Agent 在某种不被前面的检测器覆盖的模式下空转，max_turns 是兜底
成本控制：即便 Agent 真的在做有用的事，20 轮也够了。如果 20 轮还没完，说明任务可能需要拆分

关键设计：max_turns 的检查发生在工具执行完成后、下一轮 API 调用前。这意味着最后一轮的工具是会被执行的，不会出现「差一步就完成了但被硬停」的情况。

3. aborted_streaming——用户在流式输出时中断

用户按了 Esc 或者 Ctrl+C，或者手动退出。说明模型还在输出，被用户打断了。

处理方式：保留已经收到的文本，标记为「被用户中断」。

4. aborted_tools——用户在工具执行时中断

跟上一个类似，但发生在工具执行阶段。比如 Agent 正在跑一个 Bash 命令，用户按了 Esc。

这个更复杂一点：已经启动的工具可能还在后台运行（比如一个正在编译的进程），系统需要等工具完成或者超时后再退出。

5. hook_stopped——钩子触发停止

用户可以设置自定义 Hook：「每次 Agent 想执行工具的时候，先跑一下我的检查脚本」。如果脚本返回「不允许」，Agent 就停了。

典型场景：CI 环境里，Hook 检查代码是否通过 lint，不通过就阻止 Agent 继续。

6. blocking_limit——上下文快满了，提前拦截

在发 API 请求之前，系统先算一下当前上下文的 token 数。如果超过了 上下文窗口 - 一定阈值，直接不发请求，立刻退出。

一定阈值的缓冲区是为了确保判断足够保守，Claude Code 里面把这个阈值设为 3000 token。为什么不发出去让 API 自己拒绝？因为那样用户要白等一个网络往返（可能几秒钟），还可能被计费。提前拦截更快更省。

7. prompt_too_long——上下文真的满了，但还有救

blocking_limit 是客户端的估算，有时候会有误差——算出来没超，发过去 API 还是返回了 413。

但这时候 Claude Code 不会直接退出，而是做两轮恢复尝试：

Context Collapse：轻量操作，把上下文中可以折叠的部分（比如很久之前已执行完的工具结果）压缩掉。
Reactive Compact：重量级操作，调用模型对早期的对话历史做摘要压缩，把几千 token 的详细记录缩成几百 token 的摘要。

两轮都试过还是太长，才真正退出。

所以 blocking_limit 和 prompt_too_long 的关系是：blocking_limit 是预检，能挡住大部分情况；prompt_too_long 是预检漏掉之后的恢复机制，带两次自救的尝试。

设计原则：每种退出都要有上下文

不管是哪种退出方式，Agent 都应该告诉用户三件事：

停了：明确表示 Agent 已经停止工作
为什么停了：是正常完成？被用户中断？达到限制？出错了？
能做什么：是否可以继续？需要调整什么参数？

没有上下文的「已停止」是用户体验灾难。用户不知道发生了什么，不知道之前的工作有没有保存，不知道下一步该怎么办。

三个保险丝的协作

把三个保险丝和七种退出路径串在一起看：

你会发现，这些保险丝不是互斥的——它们在 Agent Loop 的不同阶段分别守护不同的风险：

工具执行后：死循环检测负责（比对调用指纹 + 返回结果，重复才干预）
模型输出后：Token 预算负责
输出异常时：截断恢复负责
全局层面：max_turns + 上下文检查兜底

这就像一栋大楼的消防系统：烟雾报警器、喷淋系统、防火门、消防栓——各管各的，但一起确保不管哪里出问题，都有人管。

动手试试：三根合体

前面分别跑了三根保险丝，现在把它们装进同一个 while (true) 里。你可以改 setScenario 切换不同的失控场景，看三根保险丝分别在什么时候触发：

src/agent.ts
复制
import { createHash } from 'node:crypto';
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { ToolCall, Message } from './types.js';

// ———— 保险丝 1：死循环检测 ————
const loopHistory = new Map<string, { count: number; lastResult: string }>();

function checkLoop(tool: ToolCall, result: string): 'ok' | 'warn' | 'break' {
  const fp = createHash('sha256')
    .update(tool.name + JSON.stringify(tool.params, Object.keys((tool.params || {}) as any).sort()))
    .digest('hex').slice(0, 12);
  const entry = loopHistory.get(fp) || { count: 0, lastResult: '' };
  if (entry.lastResult === result) entry.count++;
  else entry.count = 1;
  entry.lastResult = result;
  loopHistory.set(fp, entry);
  if (entry.count >= 10) return 'break';
  if (entry.count >= 5) return 'warn';
  return 'ok';
}

// ———— 保险丝 2：Token 预算 ————
const TOKEN_BUDGET = 50000;
let totalOutput = 0;
let lowStreak = 0;

function checkBudget(tokens: number): 'ok' | 'nudge' | 'stop' {
  totalOutput += tokens;
  if (totalOutput > 5000) {
    if (tokens < 500) lowStreak++;
    else lowStreak = 0;
    if (lowStreak >= 2) return 'stop';
  }
  if (totalOutput >= TOKEN_BUDGET * 0.9) return 'nudge';
  return 'ok';
}

// ———— 保险丝 3：截断恢复 ————
const MAX_RECOVERY = 3;
let recoveryCount = 0;

function handleTruncation(messages: Message[]): 'retry' | 'give_up' {
  recoveryCount++;
  if (recoveryCount > MAX_RECOVERY) return 'give_up';
  const msg = recoveryCount === 1
    ? '直接从断点继续——不要道歉，不要回顾。拆成更小的块。'
    : '再次被截断。大幅精简，只列关键结论。';
  console.log(`  🔄 恢复 ${recoveryCount}/${MAX_RECOVERY}`);
  messages.push({ role: 'system', content: `[TRUNCATION_RECOVERY] ${msg}` });
  return 'retry';
}

// ———— Agent Loop：三根保险丝协作 ————

setScenario('truncation'); // 试试 'dead_loop' | 'token_burn' | 'truncation'
const messages: Message[] = [
  { role: 'user', content: '分析 package.json 的所有依赖，列出需要更新的' }
];
let turn = 0;
let warnInjected = false;
let nudgeInjected = false;

console.log('--- 完整 Agent Loop（三根保险丝）---');
console.log(`场景: truncation | Token 预算: ${TOKEN_BUDGET}\n`);

while (true) {
  turn++;
  const res = await chat(messages);

  // 保险丝 3：截断恢复
  if (res.stopReason === 'max_tokens') {
    console.log(`[轮次 ${turn}] ⚠️ 截断`);
    if (handleTruncation(messages) === 'give_up') {
      console.log('\n⛔ 截断恢复失败'); break;
    }
    continue;
  }

  // 保险丝 2：Token 预算
  const budgetStatus = checkBudget(res.outputTokens);
  console.log(`[轮次 ${turn}] ${res.text.slice(0, 40)}... (+${res.outputTokens} tokens)`);

  if (budgetStatus === 'stop') {
    console.log('\n⛔ Token 递减回报'); break;
  }
  if (budgetStatus === 'nudge' && !nudgeInjected) {
    console.log('  📢 注入 nudge');
    messages.push({ role: 'system', content: '[BUDGET_NUDGE] 精简输出，给关键结论。' });
    nudgeInjected = true;
  }

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 任务完成');
    console.log(`📊 输出:\n${res.text}`);
    break;
  }

  // 保险丝 1：死循环检测
  let stopped = false;
  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    const status = checkLoop(tool, result);
    if (status === 'break') {
      console.log(`\n⛔ 死循环：${tool.name}`); stopped = true; break;
    }
    if (status === 'warn' && !warnInjected) {
      console.log('  ⚠️ 注入循环警告');
      messages.push({ role: 'system', content: '[LOOP_WARNING] 换一种方式完成任务。' });
      warnInjected = true;
    }
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
  if (stopped) break;
}

console.log(`\n--- 统计 ---`);
console.log(`总输出: ${totalOutput} tokens | 截断恢复: ${recoveryCount} 次`);

bash
复制
npx tsx src/agent.ts


试着把第一行的 setScenario('truncation') 改成 'dead_loop' 或 'token_burn'，看同一个 loop 里三根保险丝各自在什么时候、用什么方式干预。

下一篇

到这里，Agent Loop 这一章就讲完了。我们从 while(true) 的基本结构出发，讲了流式响应、API 重试、死循环检测、Token 预算——Agent 的心脏怎么跳、怎么在出问题时自我保护。

下一章我们进入 Tool System——Agent 的手脚。

第一个问题：模型是怎么"学会"调用你写的函数的？Function Calling 这个看起来很"魔法"的能力，底层到底是什么原理？

检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
模型 API 挂了怎么办？生产级容错不是加个 try-catch 这么简单
下一篇 · 第三章：Tool System —— 给 Agent 装手脚
Function Calling 与 Structured Output：模型是怎么"学会"调用你写的函数的？
编辑器


---
## 代码块


```
指纹 = SHA256(工具名 + 稳定序列化(参数))
```


```
read_file → write_file → read_file → write_file → ...
```


```typescript
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { Message } from './types.js';

setScenario('dead_loop');

const messages: Message[] = [
  { role: 'user', content: '把所有 console.log 替换成 logger.info' }
];
const MAX_TURNS = 30;
let turn = 0;

console.log('--- 裸 Agent Loop（无保险丝）---\n');

while (true) {
  turn++;
  const res = await chat(messages);
  console.log(res.text);

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 正常完成');
    break;
  }

  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    console.log(`  🔧 ${tool.name}(${JSON.stringify(tool.params)})`);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });

  if (turn >= MAX_TURNS) {
    console.log(`\n💀 跑满 ${MAX_TURNS} 轮还没停！这就是死循环。`);
    console.log('没有保险丝 → 无限循环 → 烧钱 + 用户卡死');
    break; // demo 安全阀，真实环境没有这个
  }
}
```


```bash
npx tsx src/agent.ts
```


```typescript
import { createHash } from 'node:crypto';
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { ToolCall, Message } from './types.js';

// ———— 保险丝 1：死循环检测 ————

/** 把工具名+参数哈希成指纹，用于判断是否重复 */
function fingerprint(name: string, params: unknown): string {
  const stable = JSON.stringify(params, Object.keys((params || {}) as any).sort());
  return createHash('sha256').update(name + stable).digest('hex').slice(0, 12);
}

/** 记录每个指纹的调用次数和上次结果 */
const history = new Map<string, { count: number; lastResult: string }>();

/** 检查一次工具调用是否构成死循环 */
function checkLoop(tool: ToolCall, result: string): 'ok' | 'warn' | 'break' {
  const fp = fingerprint(tool.name, tool.params);
  const entry = history.get(fp) || { count: 0, lastResult: '' };

  // 关键：同样的调用 + 同样的结果 = 无进展
  if (entry.lastResult === result) {
    entry.count++;
  } else {
    entry.count = 1; // 结果变了，重新计数
  }
  entry.lastResult = result;
  history.set(fp, entry);

  if (entry.count >= 10) return 'break'; // 硬停
  if (entry.count >= 5) return 'warn';   // 软干预
  return 'ok';
}

// ———— Agent Loop ————
setScenario('dead_loop');
const messages: Message[] = [
  { role: 'user', content: '把所有 console.log 替换成 logger.info' }
];
let turn = 0;
let warnInjected = false;

console.log('--- Agent Loop + 死循环检测 ---\n');

while (true) {
  turn++;
  const res = await chat(messages);
  console.log(`[轮次 ${turn}] ${res.text}`);

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 干预生效，Agent 换了策略并完成任务');
    break;
  }

  let stopped = false;
  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    const status = checkLoop(tool, result);

    if (status === 'break') {
      console.log(`\n⛔ 强制停止：${tool.name} 重复 10 次且结果相同`);
      console.log('💾 已执行的工具结果已保留');
      console.log('💡 建议：拆分任务或手动检查');
      stopped = true;
      break;
    }

    if (status === 'warn' && !warnInjected) {
      console.log('  ⚠️ 检测到重复！注入干预消息...');
      messages.push({
        role: 'system',
        content: '[LOOP_WARNING] 你正在反复用相同方式操作且没有进展。请换一种方式完成任务，比如用 bash + sed 批量替换。'
      });
      warnInjected = true;
    }

    console.log(`  🔧 ${tool.name} → ${result.slice(0, 50)}`);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
  if (stopped) break;
}
```


```bash
npx tsx src/agent.ts
```


```
续写第1次：+3000 Token   ✓ 正常
续写第2次：+2500 Token   ✓ 正常
续写第3次：+400 Token    ⚠️ 增量很小
续写第4次：+300 Token    ⛔ 连续两次递减，停止
```


```
200 轮 × 1000 Token = 200,000 Token
200,000 × $15 / 1,000,000 = $3
```


```typescript
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { Message } from './types.js';

// ———— 保险丝 2：Token 预算 ————

const TOKEN_BUDGET = 15000;
let totalOutput = 0;
let lowStreak = 0;

function checkBudget(tokens: number): 'ok' | 'nudge' | 'stop' {
  totalOutput += tokens;

  // 递减回报：只在已经有实质输出后才检查（避免小输出场景误触发）
  if (totalOutput > 5000) {
    if (tokens < 500) {
      lowStreak++;
    } else {
      lowStreak = 0;
    }
    if (lowStreak >= 2) return 'stop';
  }

  // 90% 预算 → 注入 nudge
  if (totalOutput >= TOKEN_BUDGET * 0.9) return 'nudge';
  return 'ok';
}

// ———— Agent Loop ————

setScenario('token_burn');
const messages: Message[] = [
  { role: 'user', content: '分析这个项目的所有模块，给出完整报告' }
];
let turn = 0;
let nudgeInjected = false;

console.log('--- 只有保险丝 2：Token 预算 ---');
console.log(`预算上限: ${TOKEN_BUDGET} tokens\n`);

while (true) {
  turn++;
  const res = await chat(messages);

  const status = checkBudget(res.outputTokens);
  console.log(`[轮次 ${turn}] +${res.outputTokens} tokens（累计 ${totalOutput}/${TOKEN_BUDGET}）`);

  if (status === 'stop') {
    console.log('\n⛔ 连续递减回报，强制停止');
    console.log(`💾 已分析 ${turn} 个模块的结果已保留`);
    break;
  }

  if (status === 'nudge' && !nudgeInjected) {
    console.log(`  📢 90% 预算已用！注入 nudge...`);
    messages.push({
      role: 'system',
      content: `[BUDGET_NUDGE] Token 预算已用 ${Math.round(totalOutput / TOKEN_BUDGET * 100)}%。请精简输出，给关键结论。继续工作——不要总结前面做过的事。`
    });
    nudgeInjected = true;
  }

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 收到 nudge 后精简收尾');
    console.log(`📊 输出: ${res.text.slice(0, 80)}...`);
    break;
  }

  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    console.log(`  🔧 ${tool.name}(${(tool.params as any).module || ''}) → ${result.slice(0, 40)}`);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
}

console.log(`\n总计输出: ${totalOutput} tokens`);
```


```bash
npx tsx src/agent.ts
```


```typescript
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { Message } from './types.js';

// ———— 保险丝 3：截断恢复 ————

const MAX_RECOVERY = 3;
let recoveryCount = 0;

function handleTruncation(messages: Message[]): 'retry' | 'give_up' {
  recoveryCount++;
  if (recoveryCount > MAX_RECOVERY) return 'give_up';

  // 第一次温和，后续更强硬
  const msg = recoveryCount === 1
    ? '直接从断点继续——不要道歉，不要回顾。把剩余工作拆成更小的块。'
    : '再次被截断。请大幅精简输出，只列关键结论。';

  console.log(`  🔄 恢复 ${recoveryCount}/${MAX_RECOVERY}: "${msg.slice(0, 25)}..."`);
  messages.push({
    role: 'system',
    content: `[TRUNCATION_RECOVERY] ${msg}`,
  });
  return 'retry';
}

// ———— Agent Loop ————

setScenario('truncation');
const messages: Message[] = [
  { role: 'user', content: '分析 package.json 的所有依赖，列出需要更新的' }
];
let turn = 0;

console.log('--- 只有保险丝 3：截断恢复 ---\n');

while (true) {
  turn++;
  const res = await chat(messages);

  if (res.stopReason === 'max_tokens') {
    console.log(`[轮次 ${turn}] ⚠️ 输出被截断 (max_tokens)`);
    const action = handleTruncation(messages);
    if (action === 'give_up') {
      console.log('\n⛔ 连续截断 3 次，放弃');
      console.log('💾 部分内容已保留，标记为"不完整"');
      break;
    }
    continue; // 带着恢复指令重试
  }

  console.log(`[轮次 ${turn}] ${res.text.slice(0, 50)}... (${res.outputTokens} tokens)`);

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 截断恢复成功！');
    console.log(`📊 输出:\n${res.text}`);
    break;
  }

  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
}

console.log(`\n截断恢复次数: ${recoveryCount}`);
```


```bash
npx tsx src/agent.ts
```


```typescript
import { createHash } from 'node:crypto';
import { chat, setScenario } from './mock-llm.js';
import { executeTool } from './mock-tools.js';
import type { ToolCall, Message } from './types.js';

// ———— 保险丝 1：死循环检测 ————
const loopHistory = new Map<string, { count: number; lastResult: string }>();

function checkLoop(tool: ToolCall, result: string): 'ok' | 'warn' | 'break' {
  const fp = createHash('sha256')
    .update(tool.name + JSON.stringify(tool.params, Object.keys((tool.params || {}) as any).sort()))
    .digest('hex').slice(0, 12);
  const entry = loopHistory.get(fp) || { count: 0, lastResult: '' };
  if (entry.lastResult === result) entry.count++;
  else entry.count = 1;
  entry.lastResult = result;
  loopHistory.set(fp, entry);
  if (entry.count >= 10) return 'break';
  if (entry.count >= 5) return 'warn';
  return 'ok';
}

// ———— 保险丝 2：Token 预算 ————
const TOKEN_BUDGET = 50000;
let totalOutput = 0;
let lowStreak = 0;

function checkBudget(tokens: number): 'ok' | 'nudge' | 'stop' {
  totalOutput += tokens;
  if (totalOutput > 5000) {
    if (tokens < 500) lowStreak++;
    else lowStreak = 0;
    if (lowStreak >= 2) return 'stop';
  }
  if (totalOutput >= TOKEN_BUDGET * 0.9) return 'nudge';
  return 'ok';
}

// ———— 保险丝 3：截断恢复 ————
const MAX_RECOVERY = 3;
let recoveryCount = 0;

function handleTruncation(messages: Message[]): 'retry' | 'give_up' {
  recoveryCount++;
  if (recoveryCount > MAX_RECOVERY) return 'give_up';
  const msg = recoveryCount === 1
    ? '直接从断点继续——不要道歉，不要回顾。拆成更小的块。'
    : '再次被截断。大幅精简，只列关键结论。';
  console.log(`  🔄 恢复 ${recoveryCount}/${MAX_RECOVERY}`);
  messages.push({ role: 'system', content: `[TRUNCATION_RECOVERY] ${msg}` });
  return 'retry';
}

// ———— Agent Loop：三根保险丝协作 ————

setScenario('truncation'); // 试试 'dead_loop' | 'token_burn' | 'truncation'
const messages: Message[] = [
  { role: 'user', content: '分析 package.json 的所有依赖，列出需要更新的' }
];
let turn = 0;
let warnInjected = false;
let nudgeInjected = false;

console.log('--- 完整 Agent Loop（三根保险丝）---');
console.log(`场景: truncation | Token 预算: ${TOKEN_BUDGET}\n`);

while (true) {
  turn++;
  const res = await chat(messages);

  // 保险丝 3：截断恢复
  if (res.stopReason === 'max_tokens') {
    console.log(`[轮次 ${turn}] ⚠️ 截断`);
    if (handleTruncation(messages) === 'give_up') {
      console.log('\n⛔ 截断恢复失败'); break;
    }
    continue;
  }

  // 保险丝 2：Token 预算
  const budgetStatus = checkBudget(res.outputTokens);
  console.log(`[轮次 ${turn}] ${res.text.slice(0, 40)}... (+${res.outputTokens} tokens)`);

  if (budgetStatus === 'stop') {
    console.log('\n⛔ Token 递减回报'); break;
  }
  if (budgetStatus === 'nudge' && !nudgeInjected) {
    console.log('  📢 注入 nudge');
    messages.push({ role: 'system', content: '[BUDGET_NUDGE] 精简输出，给关键结论。' });
    nudgeInjected = true;
  }

  if (res.stopReason === 'end_turn') {
    console.log('\n✅ 任务完成');
    console.log(`📊 输出:\n${res.text}`);
    break;
  }

  // 保险丝 1：死循环检测
  let stopped = false;
  for (const tool of res.toolCalls) {
    const result = executeTool(tool.name, tool.params as Record<string, unknown>);
    const status = checkLoop(tool, result);
    if (status === 'break') {
      console.log(`\n⛔ 死循环：${tool.name}`); stopped = true; break;
    }
    if (status === 'warn' && !warnInjected) {
      console.log('  ⚠️ 注入循环警告');
      messages.push({ role: 'system', content: '[LOOP_WARNING] 换一种方式完成任务。' });
      warnInjected = true;
    }
    messages.push({ role: 'tool', content: result });
  }
  messages.push({ role: 'assistant', content: res.text });
  if (stopped) break;
}

console.log(`\n--- 统计 ---`);
console.log(`总输出: ${totalOutput} tokens | 截断恢复: ${recoveryCount} 次`);
```


```bash
npx tsx src/agent.ts
```
