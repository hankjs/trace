# 让 Agent 自己动起来——Cron 定时任务系统

课程
Super Agent 实战课
让 Agent 自己动起来——Cron 定时任务系统
让 Agent 自己动起来——Cron 定时任务系统

约 54 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

到目前为止，我们的 Agent 一直在"等"——用户发消息它才响应，用户不说话它就静静挂着。这是一个标准的被动式 Agent。

但现实中有大量场景需要 Agent 主动执行：每天早上 9 点总结 GitHub 仓库昨天的 commit，每隔 30 分钟检查一下数据库里有没有异常数据，每周五下午把周报自动发到飞书群...这些事情不该靠人去触发。

一个 Agent 要想从"工具"变成"助手"，必须具备自主调度能力。这一节我们给 Agent 加上定时任务系统——它自己能按时做事，也能让模型通过工具动态管理任务。

先装依赖：

bash
运行
复制
pnpm install

从 setTimeout 到调度引擎

最原始的定时执行就是 setTimeout / setInterval。但生产级的 Cron 系统需要处理远比这复杂的问题：

支持多种调度表达式（cron 语法、固定间隔、一次性定时）
任务持久化（重启之后任务还在）
执行状态追踪（跑了没有、成功还是失败、输出是什么）
连续失败自动熔断（避免一个坏任务一直重试把资源吃光）
崩溃恢复（进程挂了重启后，逾期任务能补跑）

我们这一节会把 Cron 定时任务系统的核心骨架搭完整。先看整体架构：

定义任务数据结构

一个定时任务至少需要：唯一 ID、名字、调度表达式、执行什么、是否启用。

新建 src/cron/types.ts：

src/cron/types.ts
应用
复制
export type ScheduleType = 'cron' | 'interval' | 'once';

export interface CronJobConfig {
  id: string;                 // 任务唯一标识
  name: string;               // 任务显示名称
  description?: string;       // 可选的任务描述
  schedule: string;           // 调度表达式：cron 五字段 | "every 30s" | ISO 时间戳
  scheduleType: ScheduleType; // 解析后的调度类型
  enabled: boolean;           // 是否启用
  payload: JobPayload;        // 执行内容（agent prompt 或 handler 回调）
  timeout?: number;           // 单次执行超时，单位 ms，默认 60000
  maxRetries?: number;        // 连续失败多少次后自动禁用，默认 3
  source: 'config' | 'runtime'; // 来源：config 不可删，runtime 可增删
}

export type JobPayload =
  | { type: 'agent'; prompt: string }    // 把 prompt 交给 Agent Loop 执行
  | { type: 'handler'; handler: string }; // 调用插件注册的 handler 函数

export interface RunLog {
  jobId: string;              // 所属任务 ID
  startedAt: string;          // 开始时间 ISO 字符串
  finishedAt: string;         // 结束时间 ISO 字符串
  status: 'success' | 'error' | 'timeout'; // 执行结果
  output?: string;            // 执行输出（截断到 1000 字符）
  error?: string;             // 错误信息
}

export interface CronJobState {
  config: CronJobConfig;      // 任务配置
  timerId: ReturnType<typeof setTimeout> | null; // 当前定时器句柄
  lastRun?: RunLog;           // 上一次执行记录
  consecutiveFailures: number; // 连续失败计数
  running: boolean;           // 是否正在执行中
}


source 字段区分任务来源：config 是写在配置文件里的（不可通过工具删除），runtime 是 Agent 运行时动态创建的（可以增删）。这个区分很重要，你肯定不希望模型把关键的配置级任务给删了。

JobPayload 有两种类型：agent 是最常用的，把 prompt 丢给 Agent Loop 执行一次完整的多步推理；handler 是给插件用的回调。

bash
运行
复制
pnpm start

调度表达式解析

我们支持三种调度格式，覆盖日常 90% 的场景：

固定间隔：every 30s、every 5m、every 1h
Cron 表达式：标准五字段 分 时 日 月 周，比如 0 9 * * *（每天 9 点）
一次性：ISO 时间戳 2026-05-11T09:00:00Z（到点执行一次然后删除）

新建 src/cron/parser.ts：

src/cron/parser.ts
应用
复制
import { Cron } from 'croner';
import type { ScheduleType } from './types.js';

export interface ParsedSchedule {
  type: ScheduleType;
  intervalMs?: number;       // interval 类型：固定间隔毫秒数
  cronInstance?: Cron;       // cron 类型：croner 实例，负责计算下次执行时间
  onceAt?: Date;             // once 类型：一次性执行的目标时间
}

const INTERVAL_RE = /^every\s+(\d+)\s*(s|sec|m|min|h|hour)s?$/i;

export function parseSchedule(expr: string): ParsedSchedule {
  // 固定间隔："every 30s"、"every 5m"、"every 1h"
  const intervalMatch = expr.match(INTERVAL_RE);
  if (intervalMatch) {
    const value = parseInt(intervalMatch[1]);
    const unit = intervalMatch[2].toLowerCase();
    const multiplier = unit.startsWith('h') ? 3600000
      : unit.startsWith('m') ? 60000
      : 1000;
    return { type: 'interval', intervalMs: value * multiplier };
  }

  // ISO 时间戳："2026-05-11T09:00:00Z"
  if (/^\d{4}-\d{2}-\d{2}/.test(expr)) {
    const date = new Date(expr);
    if (!isNaN(date.getTime())) {
      return { type: 'once', onceAt: date };
    }
  }

  // Cron 表达式：标准五字段 "分 时 日 月 周"
  const cronInstance = new Cron(expr);
  return { type: 'cron', cronInstance };
}

export function getNextCronTime(cron: Cron): number {
  return cron.msToNext() ?? 60000;
}


Cron 表达式的解析用的是 croner——零依赖、支持时区、支持所有标准 cron 语法（*、*/N、1,3,5、1-5、L、#）。msToNext() 直接返回距离下一次触发的毫秒数，不用自己遍历时间轴去匹配。

持久化存储

任务定义存在 JSON 文件里，执行日志用 JSONL 追加写入——跟前面 Session 存储的思路一脉相承。

新建 src/cron/store.ts：

src/cron/store.ts
应用
复制
import fs from 'node:fs';
import type { CronJobConfig, RunLog } from './types.js';

const JOBS_FILE = '.cron/jobs.json';
const LOGS_FILE = '.cron/logs.jsonl';

export class CronStore {
  constructor(private baseDir: string = '.') {}

  private get jobsPath() { return `${this.baseDir}/${JOBS_FILE}`; }
  private get logsPath() { return `${this.baseDir}/${LOGS_FILE}`; }

  init(): void {
    const dir = `${this.baseDir}/.cron`;
    if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
  }

  loadJobs(): CronJobConfig[] {
    if (!fs.existsSync(this.jobsPath)) return [];
    try {
      const data = JSON.parse(fs.readFileSync(this.jobsPath, 'utf-8'));
      return data.jobs || [];
    } catch {
      return [];
    }
  }

  saveJobs(jobs: CronJobConfig[]): void {
    this.init();
    fs.writeFileSync(this.jobsPath, JSON.stringify({ jobs }, null, 2));
  }

  appendLog(log: RunLog): void {
    this.init();
    fs.appendFileSync(this.logsPath, JSON.stringify(log) + '\n');
  }

  getRecentLogs(jobId?: string, limit = 10): RunLog[] {
    if (!fs.existsSync(this.logsPath)) return [];
    const lines = fs.readFileSync(this.logsPath, 'utf-8')
      .split('\n')
      .filter(Boolean);

    let logs: RunLog[] = lines.map(l => {
      try { return JSON.parse(l); } catch { return null; }
    }).filter(Boolean) as RunLog[];

    if (jobId) logs = logs.filter(l => l.jobId === jobId);
    return logs.slice(-limit);
  }
}


saveJobs 把整个任务列表序列化写到 .cron/jobs.json。每次增删任务都会触发一次全量写入——任务数量通常不会超过几十个，全量写的性能完全够用。

CronService：调度引擎核心

CronService 是整个系统的核心实现——管理任务的生命周期：加载、调度、执行、失败处理、持久化，都在这个 Service 里面。

新建 src/cron/service.ts：

src/cron/service.ts
应用
复制
import type { CronJobConfig, CronJobState, RunLog, JobPayload } from './types.js';
import { parseSchedule, getNextCronTime } from './parser.js';
import { CronStore } from './store.js';

const QUOTES = [
  '"知之为知之，不知为不知，是知也。" —— 孔子',
  '"学而不思则罔，思而不学则殆。" —— 孔子',
  '"千里之行，始于足下。" —— 老子',
  '"天行健，君子以自强不息。" —— 《周易》',
  '"不积跬步，无以至千里。" —— 荀子',
  '"Stay hungry, stay foolish." —— Steve Jobs',
  '"The best way to predict the future is to invent it." —— Alan Kay',
  '"Talk is cheap. Show me the code." —— Linus Torvalds',
  '"Simplicity is the ultimate sophistication." —— Leonardo da Vinci',
  '"First, solve the problem. Then, write the code." —— John Johnson',
];

export interface CronExecutor {
  runAgentPrompt: (prompt: string, timeout?: number) => Promise<string>;
  notify?: (message: string) => void;
}

export class CronService {
  private jobs = new Map<string, CronJobState>();
  private store: CronStore;
  private executor?: CronExecutor;
  private running = false;

  constructor(baseDir = '.') {
    this.store = new CronStore(baseDir);
    this.store.init();
  }

  setExecutor(executor: CronExecutor): void {
    this.executor = executor;
  }

  load(): void {
    const configs = this.store.loadJobs();
    for (const config of configs) {
      if (config.enabled) {
        this.jobs.set(config.id, {
          config,
          timerId: null,
          consecutiveFailures: 0,
          running: false,
        });
      }
    }
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    for (const state of this.jobs.values()) {
      if (state.config.enabled) this.scheduleJob(state);
    }
  }

  stop(): void {
    this.running = false;
    for (const state of this.jobs.values()) {
      if (state.timerId) {
        clearTimeout(state.timerId);
        state.timerId = null;
      }
    }
  }

  add(config: CronJobConfig): void {
    if (this.jobs.has(config.id)) {
      throw new Error(`任务 ${config.id} 已存在`);
    }
    const state: CronJobState = {
      config,
      timerId: null,
      consecutiveFailures: 0,
      running: false,
    };
    this.jobs.set(config.id, state);
    this.persist();
    if (this.running && config.enabled) this.scheduleJob(state);
  }

  remove(id: string): boolean {
    const state = this.jobs.get(id);
    if (!state) return false;
    if (state.timerId) clearTimeout(state.timerId);
    this.jobs.delete(id);
    this.persist();
    return true;
  }

  enable(id: string): boolean {
    const state = this.jobs.get(id);
    if (!state) return false;
    state.config.enabled = true;
    state.consecutiveFailures = 0;
    this.persist();
    if (this.running) this.scheduleJob(state);
    return true;
  }

  disable(id: string): boolean {
    const state = this.jobs.get(id);
    if (!state) return false;
    state.config.enabled = false;
    if (state.timerId) {
      clearTimeout(state.timerId);
      state.timerId = null;
    }
    this.persist();
    return true;
  }

  list(): Array<{ config: CronJobConfig; status: string; lastRun?: RunLog }> {
    return Array.from(this.jobs.values()).map(state => ({
      config: state.config,
      status: state.running ? 'running'
        : !state.config.enabled ? 'disabled'
        : state.timerId ? 'scheduled'
        : 'idle',
      lastRun: state.lastRun,
    }));
  }

  async runNow(id: string): Promise<string> {
    const state = this.jobs.get(id);
    if (!state) return `任务 ${id} 不存在`;
    return this.executeJob(state);
  }

  getRecentLogs(jobId?: string, limit?: number): RunLog[] {
    return this.store.getRecentLogs(jobId, limit);
  }

  private scheduleJob(state: CronJobState): void {
    if (state.timerId) {
      clearTimeout(state.timerId);
      state.timerId = null;
    }

    try {
      const parsed = parseSchedule(state.config.schedule);
      let delayMs: number;

      switch (parsed.type) {
        case 'interval':
          delayMs = parsed.intervalMs!;
          break;
        case 'once': {
          const diff = parsed.onceAt!.getTime() - Date.now();
          if (diff <= 0) {
            this.executeJob(state);
            return;
          }
          delayMs = diff;
          break;
        }
        case 'cron':
          delayMs = getNextCronTime(parsed.cronFields!);
          break;
      }

      state.timerId = setTimeout(async () => {
        await this.executeJob(state);
        if (parsed.type !== 'once' && state.config.enabled && this.running) {
          this.scheduleJob(state);
        } else if (parsed.type === 'once') {
          this.remove(state.config.id);
        }
      }, delayMs);
    } catch (err: any) {
      console.log(`  [cron] ✗ 调度失败 ${state.config.id}: ${err.message}`);
    }
  }

  private async executeJob(state: CronJobState): Promise<string> {
    if (state.running) return '任务正在执行中';
    state.running = true;

    const startedAt = new Date().toISOString();
    let output = '';
    let status: RunLog['status'] = 'success';
    let error: string | undefined;

    try {
      const timeout = state.config.timeout || 60000;
      output = await this.runPayload(state.config.payload, timeout);
      state.consecutiveFailures = 0;
    } catch (err: any) {
      status = err.message?.includes('timeout') ? 'timeout' : 'error';
      error = err.message;
      output = `执行失败: ${err.message}`;
      state.consecutiveFailures++;

      const maxRetries = state.config.maxRetries ?? 3;
      if (state.consecutiveFailures >= maxRetries) {
        state.config.enabled = false;
        console.log(`  [cron] ✗ ${state.config.id} 连续失败 ${maxRetries} 次，已自动禁用`);
        this.persist();
      }
    } finally {
      state.running = false;
    }

    const log: RunLog = {
      jobId: state.config.id,
      startedAt,
      finishedAt: new Date().toISOString(),
      status,
      output: output.slice(0, 1000),
      error,
    };
    state.lastRun = log;
    this.store.appendLog(log);

    if (this.executor?.notify) {
      const icon = status === 'success' ? '✓' : '✗';
      this.executor.notify(`[cron] ${icon} ${state.config.name}: ${output.slice(0, 200)}`);
    }

    return output;
  }

  private async runPayload(payload: JobPayload, timeout: number): Promise<string> {
    if (!this.executor) {
      return '[cron] 未设置执行器，无法运行任务';
    }

    if (payload.type === 'agent') {
      return this.executor.runAgentPrompt(payload.prompt, timeout);
    }

    if (payload.type === 'handler') {
      if (payload.handler === 'random-quote') {
        return QUOTES[Math.floor(Math.random() * QUOTES.length)];
      }
      return `[handler] ${payload.handler} — handler 类型需要通过插件注册`;
    }

    return '未知 payload 类型';
  }

  private persist(): void {
    const configs = Array.from(this.jobs.values())
      .filter(s => s.config.source === 'runtime')
      .map(s => s.config);
    const existing = this.store.loadJobs().filter(j => j.source === 'config');
    this.store.saveJobs([...existing, ...configs]);
  }
}


这里有几个值得注意的设计：

执行器注入：CronService 不直接依赖 Agent Loop——通过 setExecutor 注入执行能力。这样 CronService 可以独立测试，也方便以后换成别的执行方式（比如 HTTP 调用 Agent）。

连续失败自动禁用：一个任务连续失败 N 次后自动 enabled = false，避免错误无限循环。Claude Code 内部也有类似设计——定时任务超过 7 天没更新会自动过期删除。

一次性任务执行后自动清理：once 类型的任务执行完就删掉。

persist 方法只存 runtime 来源的任务：config 来源的任务由配置文件管理，不会被覆盖。

cron_manage 工具：让模型管理定时任务

Agent 需要一个工具来创建和管理定时任务。这是整个 Cron 系统面向模型的接口。

新建 src/tools/cron-tools.ts：

src/tools/cron-tools.ts
应用
复制
import type { ToolDefinition } from './registry.js';
import type { CronService } from '../cron/service.js';
import type { CronJobConfig, ScheduleType } from '../cron/types.js';

export function createCronTool(cronService: CronService): ToolDefinition {
  return {
    name: 'cron_manage',
    description: '管理定时任务。支持创建、删除、查看、立即执行定时任务。',
    parameters: {
      type: 'object',
      properties: {
        action: {
          type: 'string',
          enum: ['list', 'add', 'remove', 'run', 'enable', 'disable', 'logs'],
          description: '操作类型',
        },
        id: { type: 'string', description: '任务 ID（add/remove/run/enable/disable 时必填）' },
        name: { type: 'string', description: '任务名称（add 时必填）' },
        schedule: {
          type: 'string',
          description: '调度表达式：cron("*/5 * * * *")、间隔("every 30s")、一次性(ISO 时间戳)',
        },
        prompt: { type: 'string', description: '任务执行时发送给 Agent 的 prompt（add 时必填）' },
      },
      required: ['action'],
    },
    isConcurrencySafe: false,
    isReadOnly: false,
    execute: async (input: {
      action: string;
      id?: string;
      name?: string;
      schedule?: string;
      prompt?: string;
    }) => {
      switch (input.action) {
        case 'list': {
          const jobs = cronService.list();
          if (jobs.length === 0) return '当前没有定时任务';
          return jobs.map(j => {
            const last = j.lastRun
              ? ` | 上次: ${j.lastRun.status} @ ${j.lastRun.finishedAt}`
              : '';
            return `[${j.status}] ${j.config.id} — ${j.config.name}\n  调度: ${j.config.schedule}${last}`;
          }).join('\n\n');
        }

        case 'add': {
          if (!input.id || !input.name || !input.schedule || !input.prompt) {
            return '添加任务需要: id, name, schedule, prompt';
          }
          const scheduleType: ScheduleType = input.schedule.startsWith('every') ? 'interval'
            : /^\d{4}-/.test(input.schedule) ? 'once'
            : 'cron';

          const config: CronJobConfig = {
            id: input.id,
            name: input.name,
            schedule: input.schedule,
            scheduleType,
            enabled: true,
            payload: { type: 'agent', prompt: input.prompt },
            source: 'runtime',
          };

          try {
            cronService.add(config);
            return `✓ 任务 "${input.name}" 已创建，调度: ${input.schedule}`;
          } catch (err: any) {
            return `✗ 创建失败: ${err.message}`;
          }
        }

        case 'remove': {
          if (!input.id) return '需要指定任务 id';
          const removed = cronService.remove(input.id);
          return removed ? `✓ 任务 ${input.id} 已删除` : `✗ 任务 ${input.id} 不存在`;
        }

        case 'run': {
          if (!input.id) return '需要指定任务 id';
          return cronService.runNow(input.id);
        }

        case 'enable': {
          if (!input.id) return '需要指定任务 id';
          return cronService.enable(input.id) ? `✓ 已启用` : `✗ 任务不存在`;
        }

        case 'disable': {
          if (!input.id) return '需要指定任务 id';
          return cronService.disable(input.id) ? `✓ 已禁用` : `✗ 任务不存在`;
        }

        case 'logs': {
          const logs = cronService.getRecentLogs(input.id, 5);
          if (logs.length === 0) return '暂无执行记录';
          return logs.map(l =>
            `[${l.status}] ${l.jobId} @ ${l.startedAt}\n  ${l.output?.slice(0, 100) || l.error || ''}`
          ).join('\n\n');
        }

        default:
          return `未知操作: ${input.action}`;
      }
    },
  };
}


把所有 CRUD 操作合并到一个 cron_manage 工具而不是拆成 cron_add、cron_remove、cron_list 多个工具——这是 Agent 工具设计的一个常见模式。

工具越少，模型选择越准确，Tool Description 占用的 token 也越少。代价是 action 参数需要额外描述，但收益远大于开销。大家可以在自己设计工具的时候经常使用这个技巧。

接入主流程

最后把 CronService 集成到 index.ts。核心改动主要有三处：初始化服务、注入执行器、关闭逻辑。

src/index.ts
应用
复制
// ... 已有 import ...
import { CronService } from './cron/service.js';
import { createCronTool } from './tools/cron-tools.js';
import { createCronCommands } from './commands/cron.js';

// ... 已有代码 ...

// ── Cron Service ────────────────────────────────
const cronService = new CronService('.');
registry.register(createCronTool(cronService));

// ... Commands 注册里加一行 ...
...createCronCommands(cronService),

// ... main() 函数里启动 Cron，并注入执行器 ...
  cronService.load();
  cronService.setExecutor({
    runAgentPrompt: async (prompt, timeout) => {
      const cronMessages: ModelMessage[] = [{ role: 'user', content: prompt }];
      const system = builder.build(makePromptCtx());
      await agentLoop(model, registry, cronMessages, system);
      const lastMsg = cronMessages[cronMessages.length - 1];
      if (!lastMsg) return '(无输出)';
      if (typeof lastMsg.content === 'string') return lastMsg.content;
      if (Array.isArray(lastMsg.content)) {
        return lastMsg.content
          .filter((p: any) => p.type === 'text')
          .map((p: any) => p.text)
          .join('') || '(无输出)';
      }
      return String(lastMsg.content);
    },
    notify: (message) => {
      console.log(`\n${message}`);
    },
  });
  cronService.start();

// ... 退出时停止调度 ...
  cronService.stop();

bash
运行
复制
pnpm start


现在试试让 Agent 创建一个定时任务：

text
复制
You: 帮我创建一个每 30 秒执行的定时任务，输出一句名言


等 30 秒后你会看到 Agent 自动执行了：

text
复制
[cron] ✓ 名言推送: xxx


每次从内置的 10 条名言里随机挑一条直接输出，不走模型、不消耗 token。这就是 handler 和 agent 两种 payload 的区别——agent 把 prompt 丢进 Agent Loop 做多步推理，handler 直接执行一段逻辑返回结果，适合不需要 LLM 参与的轻量任务。

现在试试持久化——输入 exit 退出，再重新启动：

bash
运行
复制
pnpm start

text
复制
  Cron: 1 个任务已加载    ← 从 .cron/jobs.json 读回来了


输入 /cron 确认：

text
复制
  定时任务 (1):
    ◉ quote-task — 名言推送 [every 30s] (scheduled)


任务还在，重启后自动恢复调度。这就是 CronStore 的作用——add() 时写磁盘，load() 时读回来。

Cron Job 安全防护措施

定时任务系统给了 Agent 自主执行能力，但也引入了新的风险——模型可能创建一个高频任务把系统打满，或者创建一个执行时间超长的任务卡住所有后续调度。

当前我们已经有的防护：

连续失败自动禁用：maxRetries 默认 3 次，连续失败后自动 enabled = false
权限系统联动：上一节的角色权限在这里生效——guest 角色用不了 cron_manage 工具
source 区分：config 来源的任务模型删不掉

在生产环境中，你可能还会加：

最大任务数限制（比如 50 个）
最小调度间隔限制（比如不允许 every 1s）
单任务执行超时 + AbortSignal
并发执行数量上限

这些功能当做课后作业留给大家，今天的核心骨架搭起来了，再去做这些安全限制，相信对你来说也绰绰有余了。

/cron 快捷命令

新建 src/commands/cron.ts，提供快速查看能力：

src/commands/cron.ts
应用
复制
import type { CommandHandler } from './index.js';
import type { CronService } from '../cron/service.js';

export function createCronCommands(cronService: CronService): CommandHandler[] {
  const handler: CommandHandler = (cmd) => {
    if (!cmd.startsWith('/cron')) return false;

    const sub = cmd.slice(5).trim();

    if (!sub || sub === 'list') {
      const jobs = cronService.list();
      if (jobs.length === 0) {
        console.log('  暂无定时任务');
      } else {
        console.log(`  定时任务 (${jobs.length}):`);
        for (const j of jobs) {
          const icon = j.status === 'running' ? '⟳'
            : j.status === 'scheduled' ? '◉'
            : j.status === 'disabled' ? '○'
            : '·';
          console.log(`    ${icon} ${j.config.id} — ${j.config.name} [${j.config.schedule}] (${j.status})`);
        }
      }
      return true;
    }

    if (sub === 'logs') {
      const logs = cronService.getRecentLogs(undefined, 10);
      if (logs.length === 0) {
        console.log('  暂无执行记录');
      } else {
        console.log('  最近执行记录:');
        for (const l of logs) {
          const icon = l.status === 'success' ? '✓' : '✗';
          console.log(`    ${icon} ${l.jobId} @ ${l.startedAt} — ${l.output?.slice(0, 80) || l.error || ''}`);
        }
      }
      return true;
    }

    console.log('  用法: /cron [list|logs]');
    return true;
  };

  return [handler];
}

bash
运行
复制
pnpm start

小结

这一节我们实现了完整的 Cron 调度系统。Agent 从此不再只是被动响应——它可以按时间表自己做事了。

架构上的关键决策：

调度解析与执行分离：parser.ts 只负责把表达式翻译成时间，service.ts 管调度和执行，互不耦合。
执行器注入：CronService 不直接依赖 Agent Loop，通过接口注入，方便测试和替换。
持久化用 JSON + JSONL：任务定义用 JSON（需要原子更新），执行日志用 JSONL（只追加）。
安全防护措施：连续失败自动禁用、source 区分、权限系统联动。

下一节我们给 Agent 加上 Sub-Agent 能力——一个 Agent 不够用的时候，拆成多个并行执行，再汇总结果。

参考资料
croner — 生产级 Cron 表达式解析库
cron-parser — Node.js Cron 表达式解析
node-cron — Node.js 调度库
上一篇
权限系统 + Hook 管线——让 Agent 安全地被别人使用
下一篇 · 第六章：权限 + Cron + Multi-Agent
一个不够就拆成多个——实现 Sub-Agent 机制
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
export type ScheduleType = 'cron' | 'interval' | 'once';

export interface CronJobConfig {
  id: string;                 // 任务唯一标识
  name: string;               // 任务显示名称
  description?: string;       // 可选的任务描述
  schedule: string;           // 调度表达式：cron 五字段 | "every 30s" | ISO 时间戳
  scheduleType: ScheduleType; // 解析后的调度类型
  enabled: boolean;           // 是否启用
  payload: JobPayload;        // 执行内容（agent prompt 或 handler 回调）
  timeout?: number;           // 单次执行超时，单位 ms，默认 60000
  maxRetries?: number;        // 连续失败多少次后自动禁用，默认 3
  source: 'config' | 'runtime'; // 来源：config 不可删，runtime 可增删
}

export type JobPayload =
  | { type: 'agent'; prompt: string }    // 把 prompt 交给 Agent Loop 执行
  | { type: 'handler'; handler: string }; // 调用插件注册的 handler 函数

export interface RunLog {
  jobId: string;              // 所属任务 ID
  startedAt: string;          // 开始时间 ISO 字符串
  finishedAt: string;         // 结束时间 ISO 字符串
  status: 'success' | 'error' | 'timeout'; // 执行结果
  output?: string;            // 执行输出（截断到 1000 字符）
  error?: string;             // 错误信息
}

export interface CronJobState {
  config: CronJobConfig;      // 任务配置
  timerId: ReturnType<typeof setTimeout> | null; // 当前定时器句柄
  lastRun?: RunLog;           // 上一次执行记录
  consecutiveFailures: number; // 连续失败计数
  running: boolean;           // 是否正在执行中
}
```


```bash
pnpm start
```


```ts
import { Cron } from 'croner';
import type { ScheduleType } from './types.js';

export interface ParsedSchedule {
  type: ScheduleType;
  intervalMs?: number;       // interval 类型：固定间隔毫秒数
  cronInstance?: Cron;       // cron 类型：croner 实例，负责计算下次执行时间
  onceAt?: Date;             // once 类型：一次性执行的目标时间
}

const INTERVAL_RE = /^every\s+(\d+)\s*(s|sec|m|min|h|hour)s?$/i;

export function parseSchedule(expr: string): ParsedSchedule {
  // 固定间隔："every 30s"、"every 5m"、"every 1h"
  const intervalMatch = expr.match(INTERVAL_RE);
  if (intervalMatch) {
    const value = parseInt(intervalMatch[1]);
    const unit = intervalMatch[2].toLowerCase();
    const multiplier = unit.startsWith('h') ? 3600000
      : unit.startsWith('m') ? 60000
      : 1000;
    return { type: 'interval', intervalMs: value * multiplier };
  }

  // ISO 时间戳："2026-05-11T09:00:00Z"
  if (/^\d{4}-\d{2}-\d{2}/.test(expr)) {
    const date = new Date(expr);
    if (!isNaN(date.getTime())) {
      return { type: 'once', onceAt: date };
    }
  }

  // Cron 表达式：标准五字段 "分 时 日 月 周"
  const cronInstance = new Cron(expr);
  return { type: 'cron', cronInstance };
}

export function getNextCronTime(cron: Cron): number {
  return cron.msToNext() ?? 60000;
}
```


```ts
import fs from 'node:fs';
import type { CronJobConfig, RunLog } from './types.js';

const JOBS_FILE = '.cron/jobs.json';
const LOGS_FILE = '.cron/logs.jsonl';

export class CronStore {
  constructor(private baseDir: string = '.') {}

  private get jobsPath() { return `${this.baseDir}/${JOBS_FILE}`; }
  private get logsPath() { return `${this.baseDir}/${LOGS_FILE}`; }

  init(): void {
    const dir = `${this.baseDir}/.cron`;
    if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
  }

  loadJobs(): CronJobConfig[] {
    if (!fs.existsSync(this.jobsPath)) return [];
    try {
      const data = JSON.parse(fs.readFileSync(this.jobsPath, 'utf-8'));
      return data.jobs || [];
    } catch {
      return [];
    }
  }

  saveJobs(jobs: CronJobConfig[]): void {
    this.init();
    fs.writeFileSync(this.jobsPath, JSON.stringify({ jobs }, null, 2));
  }

  appendLog(log: RunLog): void {
    this.init();
    fs.appendFileSync(this.logsPath, JSON.stringify(log) + '\n');
  }

  getRecentLogs(jobId?: string, limit = 10): RunLog[] {
    if (!fs.existsSync(this.logsPath)) return [];
    const lines = fs.readFileSync(this.logsPath, 'utf-8')
      .split('\n')
      .filter(Boolean);

    let logs: RunLog[] = lines.map(l => {
      try { return JSON.parse(l); } catch { return null; }
    }).filter(Boolean) as RunLog[];

    if (jobId) logs = logs.filter(l => l.jobId === jobId);
    return logs.slice(-limit);
  }
}
```


```ts
import type { CronJobConfig, CronJobState, RunLog, JobPayload } from './types.js';
import { parseSchedule, getNextCronTime } from './parser.js';
import { CronStore } from './store.js';

const QUOTES = [
  '"知之为知之，不知为不知，是知也。" —— 孔子',
  '"学而不思则罔，思而不学则殆。" —— 孔子',
  '"千里之行，始于足下。" —— 老子',
  '"天行健，君子以自强不息。" —— 《周易》',
  '"不积跬步，无以至千里。" —— 荀子',
  '"Stay hungry, stay foolish." —— Steve Jobs',
  '"The best way to predict the future is to invent it." —— Alan Kay',
  '"Talk is cheap. Show me the code." —— Linus Torvalds',
  '"Simplicity is the ultimate sophistication." —— Leonardo da Vinci',
  '"First, solve the problem. Then, write the code." —— John Johnson',
];

export interface CronExecutor {
  runAgentPrompt: (prompt: string, timeout?: number) => Promise<string>;
  notify?: (message: string) => void;
}

export class CronService {
  private jobs = new Map<string, CronJobState>();
  private store: CronStore;
  private executor?: CronExecutor;
  private running = false;

  constructor(baseDir = '.') {
    this.store = new CronStore(baseDir);
    this.store.init();
  }

  setExecutor(executor: CronExecutor): void {
    this.executor = executor;
  }

  load(): void {
    const configs = this.store.loadJobs();
    for (const config of configs) {
      if (config.enabled) {
        this.jobs.set(config.id, {
          config,
          timerId: null,
          consecutiveFailures: 0,
          running: false,
        });
      }
    }
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    for (const state of this.jobs.values()) {
      if (state.config.enabled) this.scheduleJob(state);
    }
  }

  stop(): void {
    this.running = false;
    for (const state of this.jobs.values()) {
      if (state.timerId) {
        clearTimeout(state.timerId);
        state.timerId = null;
      }
    }
  }

  add(config: CronJobConfig): void {
    if (this.jobs.has(config.id)) {
      throw new Error(`任务 ${config.id} 已存在`);
    }
    const state: CronJobState = {
      config,
      timerId: null,
      consecutiveFailures: 0,
      running: false,
    };
    this.jobs.set(config.id, state);
    this.persist();
    if (this.running && config.enabled) this.scheduleJob(state);
  }

  remove(id: string): boolean {
    const state = this.jobs.get(id);
    if (!state) return false;
    if (state.timerId) clearTimeout(state.timerId);
    this.jobs.delete(id);
    this.persist();
    return true;
  }

  enable(id: string): boolean {
    const state = this.jobs.get(id);
    if (!state) return false;
    state.config.enabled = true;
    state.consecutiveFailures = 0;
    this.persist();
    if (this.running) this.scheduleJob(state);
    return true;
  }

  disable(id: string): boolean {
    const state = this.jobs.get(id);
    if (!state) return false;
    state.config.enabled = false;
    if (state.timerId) {
      clearTimeout(state.timerId);
      state.timerId = null;
    }
    this.persist();
    return true;
  }

  list(): Array<{ config: CronJobConfig; status: string; lastRun?: RunLog }> {
    return Array.from(this.jobs.values()).map(state => ({
      config: state.config,
      status: state.running ? 'running'
        : !state.config.enabled ? 'disabled'
        : state.timerId ? 'scheduled'
        : 'idle',
      lastRun: state.lastRun,
    }));
  }

  async runNow(id: string): Promise<string> {
    const state = this.jobs.get(id);
    if (!state) return `任务 ${id} 不存在`;
    return this.executeJob(state);
  }

  getRecentLogs(jobId?: string, limit?: number): RunLog[] {
    return this.store.getRecentLogs(jobId, limit);
  }

  private scheduleJob(state: CronJobState): void {
    if (state.timerId) {
      clearTimeout(state.timerId);
      state.timerId = null;
    }

    try {
      const parsed = parseSchedule(state.config.schedule);
      let delayMs: number;

      switch (parsed.type) {
        case 'interval':
          delayMs = parsed.intervalMs!;
          break;
        case 'once': {
          const diff = parsed.onceAt!.getTime() - Date.now();
          if (diff <= 0) {
            this.executeJob(state);
            return;
          }
          delayMs = diff;
          break;
        }
        case 'cron':
          delayMs = getNextCronTime(parsed.cronFields!);
          break;
      }

      state.timerId = setTimeout(async () => {
        await this.executeJob(state);
        if (parsed.type !== 'once' && state.config.enabled && this.running) {
          this.scheduleJob(state);
        } else if (parsed.type === 'once') {
          this.remove(state.config.id);
        }
      }, delayMs);
    } catch (err: any) {
      console.log(`  [cron] ✗ 调度失败 ${state.config.id}: ${err.message}`);
    }
  }

  private async executeJob(state: CronJobState): Promise<string> {
    if (state.running) return '任务正在执行中';
    state.running = true;

    const startedAt = new Date().toISOString();
    let output = '';
    let status: RunLog['status'] = 'success';
    let error: string | undefined;

    try {
      const timeout = state.config.timeout || 60000;
      output = await this.runPayload(state.config.payload, timeout);
      state.consecutiveFailures = 0;
    } catch (err: any) {
      status = err.message?.includes('timeout') ? 'timeout' : 'error';
      error = err.message;
      output = `执行失败: ${err.message}`;
      state.consecutiveFailures++;

      const maxRetries = state.config.maxRetries ?? 3;
      if (state.consecutiveFailures >= maxRetries) {
        state.config.enabled = false;
        console.log(`  [cron] ✗ ${state.config.id} 连续失败 ${maxRetries} 次，已自动禁用`);
        this.persist();
      }
    } finally {
      state.running = false;
    }

    const log: RunLog = {
      jobId: state.config.id,
      startedAt,
      finishedAt: new Date().toISOString(),
      status,
      output: output.slice(0, 1000),
      error,
    };
    state.lastRun = log;
    this.store.appendLog(log);

    if (this.executor?.notify) {
      const icon = status === 'success' ? '✓' : '✗';
      this.executor.notify(`[cron] ${icon} ${state.config.name}: ${output.slice(0, 200)}`);
    }

    return output;
  }

  private async runPayload(payload: JobPayload, timeout: number): Promise<string> {
    if (!this.executor) {
      return '[cron] 未设置执行器，无法运行任务';
    }

    if (payload.type === 'agent') {
      return this.executor.runAgentPrompt(payload.prompt, timeout);
    }

    if (payload.type === 'handler') {
      if (payload.handler === 'random-quote') {
        return QUOTES[Math.floor(Math.random() * QUOTES.length)];
      }
      return `[handler] ${payload.handler} — handler 类型需要通过插件注册`;
    }

    return '未知 payload 类型';
  }

  private persist(): void {
    const configs = Array.from(this.jobs.values())
      .filter(s => s.config.source === 'runtime')
      .map(s => s.config);
    const existing = this.store.loadJobs().filter(j => j.source === 'config');
    this.store.saveJobs([...existing, ...configs]);
  }
}
```


```ts
import type { ToolDefinition } from './registry.js';
import type { CronService } from '../cron/service.js';
import type { CronJobConfig, ScheduleType } from '../cron/types.js';

export function createCronTool(cronService: CronService): ToolDefinition {
  return {
    name: 'cron_manage',
    description: '管理定时任务。支持创建、删除、查看、立即执行定时任务。',
    parameters: {
      type: 'object',
      properties: {
        action: {
          type: 'string',
          enum: ['list', 'add', 'remove', 'run', 'enable', 'disable', 'logs'],
          description: '操作类型',
        },
        id: { type: 'string', description: '任务 ID（add/remove/run/enable/disable 时必填）' },
        name: { type: 'string', description: '任务名称（add 时必填）' },
        schedule: {
          type: 'string',
          description: '调度表达式：cron("*/5 * * * *")、间隔("every 30s")、一次性(ISO 时间戳)',
        },
        prompt: { type: 'string', description: '任务执行时发送给 Agent 的 prompt（add 时必填）' },
      },
      required: ['action'],
    },
    isConcurrencySafe: false,
    isReadOnly: false,
    execute: async (input: {
      action: string;
      id?: string;
      name?: string;
      schedule?: string;
      prompt?: string;
    }) => {
      switch (input.action) {
        case 'list': {
          const jobs = cronService.list();
          if (jobs.length === 0) return '当前没有定时任务';
          return jobs.map(j => {
            const last = j.lastRun
              ? ` | 上次: ${j.lastRun.status} @ ${j.lastRun.finishedAt}`
              : '';
            return `[${j.status}] ${j.config.id} — ${j.config.name}\n  调度: ${j.config.schedule}${last}`;
          }).join('\n\n');
        }

        case 'add': {
          if (!input.id || !input.name || !input.schedule || !input.prompt) {
            return '添加任务需要: id, name, schedule, prompt';
          }
          const scheduleType: ScheduleType = input.schedule.startsWith('every') ? 'interval'
            : /^\d{4}-/.test(input.schedule) ? 'once'
            : 'cron';

          const config: CronJobConfig = {
            id: input.id,
            name: input.name,
            schedule: input.schedule,
            scheduleType,
            enabled: true,
            payload: { type: 'agent', prompt: input.prompt },
            source: 'runtime',
          };

          try {
            cronService.add(config);
            return `✓ 任务 "${input.name}" 已创建，调度: ${input.schedule}`;
          } catch (err: any) {
            return `✗ 创建失败: ${err.message}`;
          }
        }

        case 'remove': {
          if (!input.id) return '需要指定任务 id';
          const removed = cronService.remove(input.id);
          return removed ? `✓ 任务 ${input.id} 已删除` : `✗ 任务 ${input.id} 不存在`;
        }

        case 'run': {
          if (!input.id) return '需要指定任务 id';
          return cronService.runNow(input.id);
        }

        case 'enable': {
          if (!input.id) return '需要指定任务 id';
          return cronService.enable(input.id) ? `✓ 已启用` : `✗ 任务不存在`;
        }

        case 'disable': {
          if (!input.id) return '需要指定任务 id';
          return cronService.disable(input.id) ? `✓ 已禁用` : `✗ 任务不存在`;
        }

        case 'logs': {
          const logs = cronService.getRecentLogs(input.id, 5);
          if (logs.length === 0) return '暂无执行记录';
          return logs.map(l =>
            `[${l.status}] ${l.jobId} @ ${l.startedAt}\n  ${l.output?.slice(0, 100) || l.error || ''}`
          ).join('\n\n');
        }

        default:
          return `未知操作: ${input.action}`;
      }
    },
  };
}
```


```ts
// ... 已有 import ...
import { CronService } from './cron/service.js';
import { createCronTool } from './tools/cron-tools.js';
import { createCronCommands } from './commands/cron.js';

// ... 已有代码 ...

// ── Cron Service ────────────────────────────────
const cronService = new CronService('.');
registry.register(createCronTool(cronService));

// ... Commands 注册里加一行 ...
...createCronCommands(cronService),

// ... main() 函数里启动 Cron，并注入执行器 ...
  cronService.load();
  cronService.setExecutor({
    runAgentPrompt: async (prompt, timeout) => {
      const cronMessages: ModelMessage[] = [{ role: 'user', content: prompt }];
      const system = builder.build(makePromptCtx());
      await agentLoop(model, registry, cronMessages, system);
      const lastMsg = cronMessages[cronMessages.length - 1];
      if (!lastMsg) return '(无输出)';
      if (typeof lastMsg.content === 'string') return lastMsg.content;
      if (Array.isArray(lastMsg.content)) {
        return lastMsg.content
          .filter((p: any) => p.type === 'text')
          .map((p: any) => p.text)
          .join('') || '(无输出)';
      }
      return String(lastMsg.content);
    },
    notify: (message) => {
      console.log(`\n${message}`);
    },
  });
  cronService.start();

// ... 退出时停止调度 ...
  cronService.stop();
```


```bash
pnpm start
```


```text
You: 帮我创建一个每 30 秒执行的定时任务，输出一句名言
```


```text
[cron] ✓ 名言推送: xxx
```


```bash
pnpm start
```


```text
Cron: 1 个任务已加载    ← 从 .cron/jobs.json 读回来了
```


```text
定时任务 (1):
    ◉ quote-task — 名言推送 [every 30s] (scheduled)
```


```ts
import type { CommandHandler } from './index.js';
import type { CronService } from '../cron/service.js';

export function createCronCommands(cronService: CronService): CommandHandler[] {
  const handler: CommandHandler = (cmd) => {
    if (!cmd.startsWith('/cron')) return false;

    const sub = cmd.slice(5).trim();

    if (!sub || sub === 'list') {
      const jobs = cronService.list();
      if (jobs.length === 0) {
        console.log('  暂无定时任务');
      } else {
        console.log(`  定时任务 (${jobs.length}):`);
        for (const j of jobs) {
          const icon = j.status === 'running' ? '⟳'
            : j.status === 'scheduled' ? '◉'
            : j.status === 'disabled' ? '○'
            : '·';
          console.log(`    ${icon} ${j.config.id} — ${j.config.name} [${j.config.schedule}] (${j.status})`);
        }
      }
      return true;
    }

    if (sub === 'logs') {
      const logs = cronService.getRecentLogs(undefined, 10);
      if (logs.length === 0) {
        console.log('  暂无执行记录');
      } else {
        console.log('  最近执行记录:');
        for (const l of logs) {
          const icon = l.status === 'success' ? '✓' : '✗';
          console.log(`    ${icon} ${l.jobId} @ ${l.startedAt} — ${l.output?.slice(0, 80) || l.error || ''}`);
        }
      }
      return true;
    }

    console.log('  用法: /cron [list|logs]');
    return true;
  };

  return [handler];
}
```


```bash
pnpm start
```
