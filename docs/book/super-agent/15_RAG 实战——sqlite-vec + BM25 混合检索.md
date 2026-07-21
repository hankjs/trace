# RAG 实战——sqlite-vec + BM25 混合检索

课程
Super Agent 实战课
RAG 实战——sqlite-vec + BM25 混合检索
RAG 实战——sqlite-vec + BM25 混合检索

约 56 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇给 Agent 装了记忆系统——跨会话记住用户偏好、项目决策、外部资源。但记忆解决的是"Agent 自己经历过的事情"，有一类更常见的需求它解决不了：Agent 需要查阅它从未见过的文档。

你的项目有一份 500 页的部署手册，你问 Agent"上次部署出了什么事故"。Agent 没参与过那次部署，记忆里没有，上下文里也没有。它只能说"我不知道"，或者瞎编一个答案。

RAG（Retrieval-Augmented Generation）解决的就是这个问题。先检索，再生成——Agent 回答之前，先从文档库里找到相关内容，把检索结果塞进上下文，然后基于真实信息生成回答。本质上是给 Agent 发了一套"开卷考试"的参考资料。

这篇我们从零实现一个完整的 RAG 管线：分块、向量化、混合检索、结果注入。

先装依赖：

bash
运行
复制
pnpm install

RAG 的六个步骤

在动手写代码之前，先搞清楚 RAG 管线的完整流程。整体就这六步：

加载——把文档读进来。Markdown、纯文本直接读，PDF 需要解析工具（pdf-parse 等）。我们先处理最简单的 Markdown。

分块——把长文档切成小段。一次性把整个 500 页手册塞进上下文不现实（token 预算），也没必要（大部分内容跟当前问题无关）。分块的目标是让每个小段包含一个完整的语义单元。

向量化——把每个小段转成一串数字（embedding 向量）。语义相近的文本在向量空间里距离更近——"部署事故"和"上线出了问题"的向量会很接近，虽然字面完全不同。

存储——把向量和原文一起存下来。生产环境用 SQLite + sqlite-vec（向量索引）+ FTS5（全文索引），我们这里用内存数组实现——逻辑完全一样，只是换了存储介质。

检索——用户提问时，把问题也向量化，然后在存储里找最相近的片段。我们会用混合检索——向量搜索 + 关键词搜索，两条路径的结果合并排序。

注入——把检索到的片段塞进上下文，让模型基于真实内容生成回答。

好，我们现在就开始实现。

为了保证体验的完整度，右侧模板里面已经实现了本节所有的代码，你在启动项目之后可以直接享受到最终的效果，而课程内容会分不同的 step 来拆解各个部分的实现。

Step 1：文档分块

新建 src/rag/chunker.ts。分块策略直接影响检索质量——知识体系课引用的 PremAI 2026 基准测试显示，递归分块（按段落边界切分）的准确率是 69%，而语义分块（用 embedding 相似度判断主题边界）反而只有 54%。原因是语义分块的误差会累积——一个切分点判断错了，后面的都跟着错。

所以我们用递归段落分块：先按双换行切段落，段落太长再按句子切。目标是每个 chunk 大约 256 token（课程演示用，生产环境通常用 512）：

src/rag/chunker.ts
应用
复制
export interface Chunk {
  id: string;
  text: string;
  source: string;      // 来源文件
  index: number;        // 在文档中的位置
  tokenEstimate: number;
}

const TARGET_TOKENS = 256;
const CHARS_PER_TOKEN = 4;
const TARGET_CHARS = TARGET_TOKENS * CHARS_PER_TOKEN;

export function chunkDocument(source: string, text: string): Chunk[] {
  const paragraphs = text.split(/\n{2,}/);
  const chunks: Chunk[] = [];
  let current = '';
  let idx = 0;

  for (const para of paragraphs) {
    const trimmed = para.trim();
    if (!trimmed) continue;

    // 当前缓冲区 + 新段落超过目标大小，先把缓冲区存下来
    if (current.length + trimmed.length + 2 > TARGET_CHARS && current.length > 0) {
      chunks.push(makeChunk(source, current.trim(), idx++));
      current = '';
    }

    // 单个段落就超过目标大小，按句子切分
    if (trimmed.length > TARGET_CHARS) {
      // ... 按句子边界（句号、问号、感叹号）继续切分
    } else {
      current += (current ? '\n\n' : '') + trimmed;
    }
  }

  if (current.trim()) {
    chunks.push(makeChunk(source, current.trim(), idx++));
  }

  return chunks;
}


每个 chunk 带四个元数据：id（文件名 + 序号）、source（来源文件路径）、index（在文档中的位置序号）、tokenEstimate（估算 token 数）。这些元数据在检索结果展示时很有用——用户能看到信息来自哪个文件的哪个部分。

Step 2：Embedding——把文本变成向量

新建 src/rag/embedder.ts。Embedding 是 RAG 管线里最"神奇"的一步：一段中文文本进去，一串浮点数出来。这串数字编码了文本的语义信息——意思相近的文本，向量之间的夹角（cosine similarity）更小。

src/rag/embedder.ts
应用
复制
export type EmbeddingFn = (texts: string[]) => Promise<number[][]>;

export function createMockEmbedder(): EmbeddingFn {
  return async (texts: string[]) => texts.map(mockEmbed);
}

export function createDashScopeEmbedder(apiKey: string): EmbeddingFn {
  return async (texts: string[]) => {
    const resp = await fetch(
      'https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings',
      {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${apiKey}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          model: 'text-embedding-v3',
          input: texts,
          dimensions: 128,
        }),
      },
    );
    const data = await resp.json() as any;
    return data.data.map((d: any) => d.embedding as number[]);
  };
}


里面有两个 Embedder 的实现：createMockEmbedder 用确定性哈希生成假向量（WebContainer 里保证能跑），createDashScopeEmbedder 调阿里云的 text-embedding-v3 生成真实向量。

生产环境还有一些其它推荐的 embedding 模型：

OpenAI text-embedding-3-small（快速上手）
Gemini Embedding 2（全能型，文本、图片、音频、视频都支持）
Voyage-3-large（代码场景适用）。

Embedding 缓存是一个容易忽略但很重要的优化。同一段文本 + 同一个模型，生成的向量是确定性的。重复调 API 纯属浪费钱：

src/rag/embedder.ts
应用
复制
const embedCache = new Map<string, number[]>();

export async function embed(fn: EmbeddingFn, texts: string[]): Promise<number[][]> {
  const results: number[][] = new Array(texts.length);
  const uncached: { idx: number; text: string }[] = [];

  for (let i = 0; i < texts.length; i++) {
    const cached = embedCache.get(texts[i]);
    if (cached) {
      results[i] = cached;
    } else {
      uncached.push({ idx: i, text: texts[i] });
    }
  }

  if (uncached.length > 0) {
    const vectors = await fn(uncached.map(u => u.text));
    for (let i = 0; i < uncached.length; i++) {
      results[uncached[i].idx] = vectors[i];
      embedCache.set(uncached[i].text, vectors[i]);
    }
  }

  return results;
}


先查缓存，只对没见过的文本调 API。导入一个大文档时，如果有几个 chunk 内容相同（比如重复出现的免责声明），缓存能省掉重复的 API 调用。

Cosine Similarity(余弦相似度) 是比较两个向量"有多像"的标准方法——两个向量方向越接近，值越接近 1：

src/rag/embedder.ts
应用
复制
export function cosineSimilarity(a: number[], b: number[]): number {
  let dot = 0, normA = 0, normB = 0;
  for (let i = 0; i < a.length; i++) {
    dot += a[i] * b[i];
    normA += a[i] * a[i];
    normB += b[i] * b[i];
  }
  return dot / (Math.sqrt(normA) * Math.sqrt(normB) || 1);
}


20 行代码，不依赖任何库。

可能你会担心性能问题，但对于 1000 个 chunk 以内的场景，纯 JS 的实现是亚毫秒级的，完全够用。生产环境用 sqlite-vec 的 vec_distance_cosine 做向量搜索也会快很多（C 实现 + 索引加速），这个手段在超过 1000 个 chunk 之后可以考虑接入。

OpenClaw 在 sqlite-vec 加载失败时也是 fallback 到这种纯 JS cosine similarity 方案。

跑起来试试自动导入：

bash
运行
复制
pnpm start


启动时会自动扫描 docs/ 目录并导入：

  发现 2 个文档，自动导入知识库...
    api-design.md → 2 个片段
    deployment-guide.md → 2 个片段
  知识库就绪，共 4 个片段

Step 3：混合检索——两条路径合并排序

新建 src/rag/search.ts。这是整个 RAG 管线里最关键的部分。

我们先回顾一下，只用向量搜索有什么问题？比如"部署事故"能找到"上线出了问题"（语义相近），但找不到包含 pm2 stop 命令的片段（语义隔的比较远但关键词命中）。反过来，只用关键词搜索，就很容易漏掉语义相关的内容了。

混合检索的核心思路：跑两条搜索路径，各取所长，合并排序。

OpenClaw 的默认配比是 70% 向量权重 + 30% 关键词权重。向量搜索聚焦语义相关的召回链路，而关键词搜索聚焦精确匹配的片段召回。两者是互补的：

src/rag/search.ts
应用
复制
const VECTOR_WEIGHT = 0.7;
const KEYWORD_WEIGHT = 0.3;
const CANDIDATE_MULTIPLIER = 4;

export async function hybridSearch(
  store: VectorStore,
  embedFn: EmbeddingFn,
  query: string,
  topK: number = 5,
): Promise<SearchResult[]> {
  const all = store.getAll();
  const candidateCount = Math.min(topK * CANDIDATE_MULTIPLIER, all.length);

  // 路径 1: 向量搜索
  const [queryVec] = await embed(embedFn, [query]);
  const vectorResults = all
    .map(chunk => ({ chunk, score: cosineSimilarity(queryVec, chunk.embedding) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, candidateCount);

  // 路径 2: 关键词搜索 (BM25)
  const queryTerms = tokenize(query);
  const keywordResults = all
    .map(chunk => ({ chunk, score: bm25Score(queryTerms, chunk.text, ...) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, candidateCount);

  // 归一化 + 合并
  // ...
}


CANDIDATE_MULTIPLIER = 4 是一个重要的参数——如果最终要返回 5 条结果，先从两条路径各取 20 条候选。知识体系课讲过这个场景，原因是向量搜索的 top-5 和关键词搜索的 top-5 可能是完全不同的文档集。如果各取 5 条就合并，好结果可能在某一条路径里排第 6 被截断了。

归一化：两种分数不能直接相加

向量搜索返回 cosine similarity（0 到 1），BM25 返回的分数范围不固定——可能是负数、可能是几十。如果不归一化就按权重加，关键词搜索的分数可能直接压过向量搜索。

两条路径都用 min-max 归一化（映射到 0-1）：

src/rag/search.ts
应用
复制
function normalizeMinMax(scores: number[]): number[] {
  const min = Math.min(...scores);
  const max = Math.max(...scores);
  const range = max - min || 1;
  return scores.map(s => (s - min) / range);
}


你可能见过用 sigmoid（1 / (1 + e^-s)）来归一化 BM25 的做法。这在原始 BM25 分数（范围可能是 -5 到 20）上是合理的——sigmoid 把任意实数压到 (0, 1)。但我们的 keywordSearch 已经把 BM25 rank 转换成了 [0, 1) 区间的相似度分数，再过一次 sigmoid 只会映射到 ~0.5-0.7 的窄区间。举个例子：三个分数 [0.3, 0.5, 0.7] 过 sigmoid 变成 [0.574, 0.622, 0.668]，跨度才 0.094——乘以 0.3 的权重后，关键词路径对最终排序几乎没有影响，等于"失声"了。所以对已经在 [0, 1) 区间的分数，直接用 min-max 把区分度拉满是更好的选择。

归一化之后就可以按权重合并了。如果一个 chunk 同时出现在两条路径的结果里，它的分数是 vectorScore * 0.7 + keywordScore * 0.3——两条路径都认可的文档得分最高。

MMR 去重：避免返回高度相似的结果

混合检索之后，前几名可能是同一个话题的不同段落——内容高度重复。MMR（Maximal Marginal Relevance）在选结果时兼顾相关性和多样性：

src/rag/search.ts
应用
复制
const MMR_LAMBDA = 0.7;  // 70% 看相关性，30% 看多样性

export function mmrSelect(results: SearchResult[], topK: number): SearchResult[] {
  const selected: SearchResult[] = [results[0]];  // 第一名直接入选
  const remaining = results.slice(1);

  while (selected.length < topK && remaining.length > 0) {
    let bestIdx = 0;
    let bestMmr = -Infinity;

    for (let i = 0; i < remaining.length; i++) {
      const relevance = remaining[i].score;
      const maxSim = Math.max(
        ...selected.map(s => jaccardSimilarity(s.chunk.text, remaining[i].chunk.text))
      );
      const mmr = MMR_LAMBDA * relevance - (1 - MMR_LAMBDA) * maxSim;
      if (mmr > bestMmr) { bestMmr = mmr; bestIdx = i; }
    }

    selected.push(remaining[bestIdx]);
    remaining.splice(bestIdx, 1);
  }

  return selected;
}


每次选下一个结果时，计算 MMR = 0.7 × 相关性 - 0.3 × 与已选结果的最大相似度。这样即使一个候选的相关性分很高，如果它跟已选的某个结果太像，也会被惩罚。

多样性度量用的是 Jaccard similarity（两个文本的词集交集/并集，知识体系课的 RAG 检索优化小节里面讲过），而不是再算一次 embedding cosine——零额外 API 成本。OpenClaw 也是这个策略。

Step 4：注册 RAG 工具

RAG 管线做好了，需要作为工具注册给 Agent。跟记忆系统那节一样，用工厂函数封装到 src/tools/rag-tools.ts——两个工具：rag_ingest（导入文档）和 rag_search（搜索知识库）：

src/tools/rag-tools.ts
应用
复制
export function createRagTools(vectorStore: VectorStore, embedFn: EmbeddingFn): ToolDefinition[] {
  const ragIngestTool: ToolDefinition = {
  name: 'rag_ingest',
  description: '将文档导入知识库。内容会被分块、向量化后存储。',
  parameters: {
    type: 'object',
    properties: { path: { type: 'string', description: '文档路径' } },
    required: ['path'],
    additionalProperties: false,
  },
  execute: async ({ path }: { path: string }) => {
    const text = fs.readFileSync(path, 'utf-8');
    const chunks = chunkDocument(path, text);
    const embeddings = await embed(embedFn, chunks.map(c => c.text));
    vectorStore.addBatch(chunks.map((c, i) => ({ chunk: c, embedding: embeddings[i] })));
    return `已导入 ${chunks.length} 个文档片段。知识库共 ${vectorStore.size()} 个片段。`;
  },
};

const ragSearchTool: ToolDefinition = {
  name: 'rag_search',
  description: '从知识库中搜索相关信息。返回最相关的文档片段。',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '搜索查询' },
      top_k: { type: 'number', description: '返回结果数量（默认 5）' },
    },
    required: ['query'],
    additionalProperties: false,
  },
  execute: async ({ query, top_k }: { query: string; top_k?: number }) => {
    if (vectorStore.size() === 0) return '知识库为空，请先导入文档。';
    const results = await hybridSearch(vectorStore, embedFn, query, top_k || 5);
    return results.map((r, i) =>
      `[${i + 1}] 来源: ${r.chunk.source} | 分数: ${r.score.toFixed(3)}\n${r.chunk.text.slice(0, 500)}`
    ).join('\n\n---\n\n');
  },
  };

  return [ragIngestTool, ragSearchTool];
}


rag_search 的结果格式刻意展示了每条结果的来源和综合分——Agent 能看到信息从哪来，帮它判断可信度。结果文本截断到 500 字符是为了控制上下文预算，跟前面讲的"入口管理"一脉相承。

现在跑起来试试：

bash
运行
复制
pnpm start


试试检索效果：

You: 上次部署出了什么问题


Agent 会从文档里找到事故记录并基于它生成回答——而不是编造一个不存在的事故。

知识库状态注入 Prompt Pipe

跟上一篇的记忆注入一样，知识库的状态信息也通过 Prompt Pipe 注入 system prompt。现在有两个 pipe 了（记忆 + 知识库），把它们拆到 src/context/prompt-pipes.ts 里：

src/context/prompt-pipes.ts
应用
复制
import type { MemoryStore } from '../memory/store.js';
import type { VectorStore } from '../rag/store.js';
import type { PromptContext } from './prompt-builder.js';

export function memoryContext(memoryStore: MemoryStore): (ctx: PromptContext) => string | null {
  return () => memoryStore.buildPromptSection();
}

export function ragContext(vectorStore: VectorStore): (ctx: PromptContext) => string | null {
  return () => {
    const size = vectorStore.size();
    if (size === 0) return null;
    const sources = vectorStore.sources();
    return `[知识库] 已导入 ${size} 个文档片段（来源: ${sources.join(', ')}）。使用 rag_search 工具搜索知识库。`;
  };
}


上一节 memoryContext 直接写在 index.ts 里（就一行闭包，不值得建文件）。现在多了 ragContext，两个 pipe 放一起更清晰。

这条 ragContext 告诉 Agent"你有知识库可用"以及"知识库里有哪些来源"。如果没有这条提示，Agent 不知道应该用 rag_search 工具——它会直接尝试回答，然后编造答案。

生产级 RAG：SQLite 三表架构

我们用内存数组实现了 RAG 管线，搜索逻辑跟生产方案完全一样。但内存方案有一个明显的局限：进程一退出，知识库就没了。每次启动都要重新分块、重新调 embedding API。文档少的时候无所谓，文档多了就不行了。

生产环境推荐 SQLite + sqlite-vec + FTS5 三表架构——这也是 OpenClaw 的做法。一个 .db 文件，不需要额外的数据库服务。

三张表各管什么

想象一下，你有 1 万个文档片段，每个片段有三样东西要存：原文内容、embedding 向量、以及来源和时间等元数据。最直觉的做法是一张表全存了。但搜索的时候会遇到两个问题：

问题一：向量搜索慢。 1 万个 128 维向量，每次搜索都要逐条算 cosine similarity——跟我们内存方案的 逐条遍历 一样。1 万条还凑合，10 万条就不行了。你需要一个专门为向量相似度优化的索引结构。

问题二：关键词搜索慢。 用 LIKE '%部署%' 做全表扫描，10 万条文本每次都要逐字符匹配。你需要一个倒排索引来加速关键词查找。

所以拆成三张表——一张主表存数据，两张索引表加速搜索。下面这张图把三表的分工和读写路径画清楚了：

简单说：

chunks 表是数据源头，存原文、向量 JSON、来源、模型名、时间戳。备份、调试、导出都查这张表。
chunks_vec 是 sqlite-vec 提供的虚拟表，把向量存成二进制并做索引加速，专门服务向量相似度搜索。可以把它理解成 chunks 表 embedding 列的专用搜索索引。
chunks_fts 是 FTS5 虚拟表，建倒排索引 + BM25 排序，专门服务关键词搜索。同样是 chunks 表 text 列的专用搜索索引。

三张表通过 chunk ID 关联。写入时三表同步，搜索时两路并行查索引、回主表取原文。

有一点需要注意：不同 embedding 模型生成的向量在不同的空间里，不能混用。 如果你从 text-embedding-3-small 换到 text-embedding-v3，必须重新生成所有 embedding，不能只对新文档用新模型。下面的实现代码里会看到 model 字段就是为了标记这个。

本地接入 SQLite：动手试试

课程里的 WebContainer 环境没法加载 native 扩展，所以用的是内存方案。但如果你想在本地试试真正的 SQLite 三表架构，操作并不复杂。

先把右侧编辑器里的代码下载到本地（或者直接把 src/ 目录的内容复制到一个新项目里），然后装两个额外的依赖：

bash
复制
pnpm add better-sqlite3 sqlite-vec
pnpm add -D @types/better-sqlite3


better-sqlite3 是 Node.js 里最快的 SQLite 库（同步 API，不需要 async/await），sqlite-vec 是向量搜索扩展。

装完之后多跑一步——better-sqlite3 是 native 模块，pnpm 出于安全考虑默认不执行 postinstall 脚本，导致 .node 二进制 binding 没被下载。需要手动批准：

bash
复制
pnpm approve-builds   # 在交互界面里勾选 better-sqlite3，回车
pnpm install           # 重新安装时会下载 binding


如果跑 pnpm start 时还看到 Could not locate the bindings file 错误，说明 prebuild binary 跟你的 Node 版本不匹配（比如 Node 23 跟旧 prebuild 不兼容）。直接 rebuild 从源码编译就行：

bash
复制
pnpm rebuild better-sqlite3


然后新建一个 src/rag/sqlite-store.ts，替换掉内存版的 VectorStore：

src/rag/sqlite-store.ts
应用
复制
import Database from 'better-sqlite3';
import * as sqliteVec from 'sqlite-vec';
import type { Chunk } from './chunker.js';
import type { StoredChunk } from './store.js';

export class SqliteVectorStore {
  private db: Database.Database;

  constructor(dbPath: string = 'knowledge.db') {
    this.db = new Database(dbPath);
    sqliteVec.load(this.db);       // 加载向量搜索扩展
    this.createTables();
  }

  private createTables() {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS chunks (
        id TEXT PRIMARY KEY,
        text TEXT NOT NULL,
        source TEXT NOT NULL,
        chunk_index INTEGER NOT NULL,
        embedding TEXT NOT NULL,
        model TEXT NOT NULL DEFAULT 'text-embedding-v3',
        updated_at INTEGER NOT NULL
      );

      CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(
        id TEXT PRIMARY KEY,
        embedding FLOAT[128]
      );

      CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
        text, id UNINDEXED, source UNINDEXED
      );
    `);
  }

  add(chunk: Chunk, embedding: number[]): void {
    const now = Date.now();
    // 三表联动写入
    this.db.prepare(`INSERT OR REPLACE INTO chunks
      (id, text, source, chunk_index, embedding, updated_at)
      VALUES (?, ?, ?, ?, ?, ?)`)
      .run(chunk.id, chunk.text, chunk.source, chunk.index,
           JSON.stringify(embedding), now);

    this.db.prepare(`INSERT OR REPLACE INTO chunks_vec (id, embedding)
      VALUES (?, ?)`)
      .run(chunk.id, Buffer.from(new Float32Array(embedding).buffer));

    this.db.prepare(`INSERT OR REPLACE INTO chunks_fts (id, text, source)
      VALUES (?, ?, ?)`)
      .run(chunk.id, chunk.text, chunk.source);
  }

  addBatch(items: Array<{ chunk: Chunk; embedding: number[] }>): void {
    const tx = this.db.transaction(() => {
      for (const { chunk, embedding } of items) this.add(chunk, embedding);
    });
    tx();  // 事务批量写入，比逐条快很多
  }

  vectorSearch(queryEmbedding: number[], topK: number): Array<{ chunk: StoredChunk; score: number }> {
    const buf = Buffer.from(new Float32Array(queryEmbedding).buffer);
    const rows = this.db.prepare(`
      SELECT v.id, v.distance, c.text, c.source, c.chunk_index, c.embedding
      FROM chunks_vec v
      JOIN chunks c ON c.id = v.id
      WHERE v.embedding MATCH ?
      ORDER BY v.distance
      LIMIT ?
    `).all(buf, topK) as any[];

    return rows.map(r => ({
      chunk: {
        id: r.id, text: r.text, source: r.source,
        index: r.chunk_index,
        tokenEstimate: Math.ceil(r.text.length / 4),
        embedding: JSON.parse(r.embedding),
        addedAt: 0,
      },
      score: 1 - r.distance,  // cosine distance → similarity
    }));
  }

  keywordSearch(query: string, topK: number): Array<{ chunk: StoredChunk; score: number }> {
    const rows = this.db.prepare(`
      SELECT f.id, bm25(chunks_fts) AS rank, c.text, c.source, c.chunk_index, c.embedding
      FROM chunks_fts f
      JOIN chunks c ON c.id = f.id
      WHERE chunks_fts MATCH ?
      ORDER BY rank
      LIMIT ?
    `).all(query, topK) as any[];

    return rows.map(r => ({
      chunk: {
        id: r.id, text: r.text, source: r.source,
        index: r.chunk_index,
        tokenEstimate: Math.ceil(r.text.length / 4),
        embedding: JSON.parse(r.embedding),
        addedAt: 0,
      },
      score: r.rank < 0 ? -r.rank / (1 - r.rank) : 1 / (1 + r.rank),
    }));
  }

  size(): number {
    return (this.db.prepare('SELECT COUNT(*) as n FROM chunks').get() as any).n;
  }

  clear(): void {
    this.db.exec('DELETE FROM chunks; DELETE FROM chunks_vec; DELETE FROM chunks_fts;');
  }

  sources(): string[] {
    return (this.db.prepare('SELECT DISTINCT source FROM chunks').all() as any[]).map(r => r.source);
  }

  // 混合搜索：直接在 SQLite 层完成向量 + 关键词双路检索
  async hybridSearch(
    embedFn: EmbeddingFn,
    query: string,
    topK: number = 5,
  ): Promise<SearchResult[]> {
    const candidateCount = Math.min(topK * 4, this.size());
    if (candidateCount === 0) return [];

    const [queryVec] = await embed(embedFn, [query]);

    // 路径 1: sqlite-vec 向量搜索
    const vectorResults = this.vectorSearch(queryVec, candidateCount);

    // 路径 2: FTS5 关键词搜索
    const keywordResults = this.keywordSearch(query, candidateCount);

    // 归一化 + 加权合并
    const vecScores = normalizeMinMax(vectorResults.map(r => r.score));
    const kwScores = normalizeMinMax(keywordResults.map(r => r.score));

    const candidates = new Map<string, SearchResult>();
    for (let i = 0; i < vectorResults.length; i++) {
      const id = vectorResults[i].chunk.id;
      candidates.set(id, {
        chunk: vectorResults[i].chunk,
        score: vecScores[i] * 0.7,
        vectorScore: vecScores[i],
        keywordScore: 0,
      });
    }
    for (let i = 0; i < keywordResults.length; i++) {
      const id = keywordResults[i].chunk.id;
      const existing = candidates.get(id);
      if (existing) {
        existing.keywordScore = kwScores[i];
        existing.score += kwScores[i] * 0.3;
      } else {
        candidates.set(id, {
          chunk: keywordResults[i].chunk,
          score: kwScores[i] * 0.3,
          vectorScore: 0,
          keywordScore: kwScores[i],
        });
      }
    }

    const sorted = [...candidates.values()]
      .sort((a, b) => b.score - a.score);

    // MMR deduplication
    return mmrSelect(sorted, topK);
  }
}

function normalizeMinMax(scores: number[]): number[] {
  if (scores.length === 0) return [];
  const min = Math.min(...scores);
  const max = Math.max(...scores);
  const range = max - min || 1;
  return scores.map(s => (s - min) / range);
}


注意 hybridSearch 方法里用到了几个额外的 import，在文件顶部补上：

typescript
复制
import { embed, type EmbeddingFn } from './embedder.js';
import { mmrSelect, type SearchResult } from './search.js';


然后修改 src/tools/rag-tools.ts，让它用 SqliteVectorStore 自带的 hybridSearch，不再依赖内存版的 search.ts：

src/tools/rag-tools.ts
应用
复制
// 替换这两行 import：
// import { VectorStore } from '../rag/store.js';
// import { hybridSearch } from '../rag/search.js';
import { SqliteVectorStore } from '../rag/sqlite-store.js';

// 函数签名也要改：
export function createRagTools(vectorStore: SqliteVectorStore, embedFn: EmbeddingFn): ToolDefinition[] {
  // ...rag_ingest 不变...

  // rag_search 的 execute 里改成：
  const results = await vectorStore.hybridSearch(embedFn, query, top_k || 5);
  // ...其余不变...
}


最后在 src/index.ts 里替换 store：

typescript
复制
// 替换这一行：
// import { VectorStore } from './rag/store.js';
import { SqliteVectorStore } from './rag/sqlite-store.js';

// 替换这一行：
// const vectorStore = new VectorStore();
const vectorStore = new SqliteVectorStore('knowledge.db');


跑一下 pnpm start，导入文档后退出，再重新启动——知识库还在，不需要重新导入。这就是持久化的好处。

Agentic RAG：不只是"搜一次"

到目前为止我们做的是传统 RAG——用户提问，搜一次，注入结果，生成回答。但有些问题一次搜索不够。

"对比 API 设计规范和部署指南中关于错误处理的差异"——这个问题需要搜两次：一次搜 API 设计文档的错误处理，一次搜部署指南的错误处理，然后综合回答。

传统 RAG 会把整个问题作为一个 query 去搜，可能两个都没搜到。Agentic RAG 不一样——Agent 自己决定搜什么、搜几次、怎么组合。 它可以先搜 "API 错误处理"，再搜 "部署 错误处理"，把两次结果放一起分析。

这不需要改 RAG 管线的代码——Agent Loop 本身就支持多步工具调用。Agent 调一次 rag_search，看到结果不够，再调一次换个关键词。这就是 Agentic RAG 的本质：RAG 管线是工具，Agent Loop 是使用工具的决策者。

其实我们这个 Agent 已经是这么实现了。

到这里，Agent 的知识获取能力从三个层面完成了搭建：上下文（Session + Compaction + Defense，管理一次对话内的信息）、记忆（上一篇，跨会话持久化经验），以及知识库（这一篇，从外部文档检索信息）。

下一篇进入记忆维护——记忆不是存了就不管的。过期的记忆会让 Agent 基于错误信息做出自信的错误决策，比没有记忆还危险。怎么检测过期？怎么自动清理？怎么防止记忆爆炸？

参考链接
RAG 全流程（知识体系课）
检索优化：语义相似 ≠ 任务相关（知识体系课）
sqlite-vec — Vector Search SQLite Extension
PremAI Chunking Benchmarks 2026
Best Embedding Models for RAG 2026
DashScope Embedding API 文档
上一篇
关掉终端再打开，Agent 还记得你是谁——持久化记忆系统
下一篇 · 第四章：Memory + RAG
记忆会变坏——给 Agent 的记忆库做体检
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
export interface Chunk {
  id: string;
  text: string;
  source: string;      // 来源文件
  index: number;        // 在文档中的位置
  tokenEstimate: number;
}

const TARGET_TOKENS = 256;
const CHARS_PER_TOKEN = 4;
const TARGET_CHARS = TARGET_TOKENS * CHARS_PER_TOKEN;

export function chunkDocument(source: string, text: string): Chunk[] {
  const paragraphs = text.split(/\n{2,}/);
  const chunks: Chunk[] = [];
  let current = '';
  let idx = 0;

  for (const para of paragraphs) {
    const trimmed = para.trim();
    if (!trimmed) continue;

    // 当前缓冲区 + 新段落超过目标大小，先把缓冲区存下来
    if (current.length + trimmed.length + 2 > TARGET_CHARS && current.length > 0) {
      chunks.push(makeChunk(source, current.trim(), idx++));
      current = '';
    }

    // 单个段落就超过目标大小，按句子切分
    if (trimmed.length > TARGET_CHARS) {
      // ... 按句子边界（句号、问号、感叹号）继续切分
    } else {
      current += (current ? '\n\n' : '') + trimmed;
    }
  }

  if (current.trim()) {
    chunks.push(makeChunk(source, current.trim(), idx++));
  }

  return chunks;
}
```


```typescript
export type EmbeddingFn = (texts: string[]) => Promise<number[][]>;

export function createMockEmbedder(): EmbeddingFn {
  return async (texts: string[]) => texts.map(mockEmbed);
}

export function createDashScopeEmbedder(apiKey: string): EmbeddingFn {
  return async (texts: string[]) => {
    const resp = await fetch(
      'https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings',
      {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${apiKey}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          model: 'text-embedding-v3',
          input: texts,
          dimensions: 128,
        }),
      },
    );
    const data = await resp.json() as any;
    return data.data.map((d: any) => d.embedding as number[]);
  };
}
```


```typescript
const embedCache = new Map<string, number[]>();

export async function embed(fn: EmbeddingFn, texts: string[]): Promise<number[][]> {
  const results: number[][] = new Array(texts.length);
  const uncached: { idx: number; text: string }[] = [];

  for (let i = 0; i < texts.length; i++) {
    const cached = embedCache.get(texts[i]);
    if (cached) {
      results[i] = cached;
    } else {
      uncached.push({ idx: i, text: texts[i] });
    }
  }

  if (uncached.length > 0) {
    const vectors = await fn(uncached.map(u => u.text));
    for (let i = 0; i < uncached.length; i++) {
      results[uncached[i].idx] = vectors[i];
      embedCache.set(uncached[i].text, vectors[i]);
    }
  }

  return results;
}
```


```typescript
export function cosineSimilarity(a: number[], b: number[]): number {
  let dot = 0, normA = 0, normB = 0;
  for (let i = 0; i < a.length; i++) {
    dot += a[i] * b[i];
    normA += a[i] * a[i];
    normB += b[i] * b[i];
  }
  return dot / (Math.sqrt(normA) * Math.sqrt(normB) || 1);
}
```


```bash
pnpm start
```


```
发现 2 个文档，自动导入知识库...
    api-design.md → 2 个片段
    deployment-guide.md → 2 个片段
  知识库就绪，共 4 个片段
```


```typescript
const VECTOR_WEIGHT = 0.7;
const KEYWORD_WEIGHT = 0.3;
const CANDIDATE_MULTIPLIER = 4;

export async function hybridSearch(
  store: VectorStore,
  embedFn: EmbeddingFn,
  query: string,
  topK: number = 5,
): Promise<SearchResult[]> {
  const all = store.getAll();
  const candidateCount = Math.min(topK * CANDIDATE_MULTIPLIER, all.length);

  // 路径 1: 向量搜索
  const [queryVec] = await embed(embedFn, [query]);
  const vectorResults = all
    .map(chunk => ({ chunk, score: cosineSimilarity(queryVec, chunk.embedding) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, candidateCount);

  // 路径 2: 关键词搜索 (BM25)
  const queryTerms = tokenize(query);
  const keywordResults = all
    .map(chunk => ({ chunk, score: bm25Score(queryTerms, chunk.text, ...) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, candidateCount);

  // 归一化 + 合并
  // ...
}
```


```typescript
function normalizeMinMax(scores: number[]): number[] {
  const min = Math.min(...scores);
  const max = Math.max(...scores);
  const range = max - min || 1;
  return scores.map(s => (s - min) / range);
}
```


```typescript
const MMR_LAMBDA = 0.7;  // 70% 看相关性，30% 看多样性

export function mmrSelect(results: SearchResult[], topK: number): SearchResult[] {
  const selected: SearchResult[] = [results[0]];  // 第一名直接入选
  const remaining = results.slice(1);

  while (selected.length < topK && remaining.length > 0) {
    let bestIdx = 0;
    let bestMmr = -Infinity;

    for (let i = 0; i < remaining.length; i++) {
      const relevance = remaining[i].score;
      const maxSim = Math.max(
        ...selected.map(s => jaccardSimilarity(s.chunk.text, remaining[i].chunk.text))
      );
      const mmr = MMR_LAMBDA * relevance - (1 - MMR_LAMBDA) * maxSim;
      if (mmr > bestMmr) { bestMmr = mmr; bestIdx = i; }
    }

    selected.push(remaining[bestIdx]);
    remaining.splice(bestIdx, 1);
  }

  return selected;
}
```


```typescript
export function createRagTools(vectorStore: VectorStore, embedFn: EmbeddingFn): ToolDefinition[] {
  const ragIngestTool: ToolDefinition = {
  name: 'rag_ingest',
  description: '将文档导入知识库。内容会被分块、向量化后存储。',
  parameters: {
    type: 'object',
    properties: { path: { type: 'string', description: '文档路径' } },
    required: ['path'],
    additionalProperties: false,
  },
  execute: async ({ path }: { path: string }) => {
    const text = fs.readFileSync(path, 'utf-8');
    const chunks = chunkDocument(path, text);
    const embeddings = await embed(embedFn, chunks.map(c => c.text));
    vectorStore.addBatch(chunks.map((c, i) => ({ chunk: c, embedding: embeddings[i] })));
    return `已导入 ${chunks.length} 个文档片段。知识库共 ${vectorStore.size()} 个片段。`;
  },
};

const ragSearchTool: ToolDefinition = {
  name: 'rag_search',
  description: '从知识库中搜索相关信息。返回最相关的文档片段。',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '搜索查询' },
      top_k: { type: 'number', description: '返回结果数量（默认 5）' },
    },
    required: ['query'],
    additionalProperties: false,
  },
  execute: async ({ query, top_k }: { query: string; top_k?: number }) => {
    if (vectorStore.size() === 0) return '知识库为空，请先导入文档。';
    const results = await hybridSearch(vectorStore, embedFn, query, top_k || 5);
    return results.map((r, i) =>
      `[${i + 1}] 来源: ${r.chunk.source} | 分数: ${r.score.toFixed(3)}\n${r.chunk.text.slice(0, 500)}`
    ).join('\n\n---\n\n');
  },
  };

  return [ragIngestTool, ragSearchTool];
}
```


```bash
pnpm start
```


```
You: 上次部署出了什么问题
```


```typescript
import type { MemoryStore } from '../memory/store.js';
import type { VectorStore } from '../rag/store.js';
import type { PromptContext } from './prompt-builder.js';

export function memoryContext(memoryStore: MemoryStore): (ctx: PromptContext) => string | null {
  return () => memoryStore.buildPromptSection();
}

export function ragContext(vectorStore: VectorStore): (ctx: PromptContext) => string | null {
  return () => {
    const size = vectorStore.size();
    if (size === 0) return null;
    const sources = vectorStore.sources();
    return `[知识库] 已导入 ${size} 个文档片段（来源: ${sources.join(', ')}）。使用 rag_search 工具搜索知识库。`;
  };
}
```


```bash
pnpm add better-sqlite3 sqlite-vec
pnpm add -D @types/better-sqlite3
```


```bash
pnpm approve-builds   # 在交互界面里勾选 better-sqlite3，回车
pnpm install           # 重新安装时会下载 binding
```


```bash
pnpm rebuild better-sqlite3
```


```typescript
import Database from 'better-sqlite3';
import * as sqliteVec from 'sqlite-vec';
import type { Chunk } from './chunker.js';
import type { StoredChunk } from './store.js';

export class SqliteVectorStore {
  private db: Database.Database;

  constructor(dbPath: string = 'knowledge.db') {
    this.db = new Database(dbPath);
    sqliteVec.load(this.db);       // 加载向量搜索扩展
    this.createTables();
  }

  private createTables() {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS chunks (
        id TEXT PRIMARY KEY,
        text TEXT NOT NULL,
        source TEXT NOT NULL,
        chunk_index INTEGER NOT NULL,
        embedding TEXT NOT NULL,
        model TEXT NOT NULL DEFAULT 'text-embedding-v3',
        updated_at INTEGER NOT NULL
      );

      CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(
        id TEXT PRIMARY KEY,
        embedding FLOAT[128]
      );

      CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
        text, id UNINDEXED, source UNINDEXED
      );
    `);
  }

  add(chunk: Chunk, embedding: number[]): void {
    const now = Date.now();
    // 三表联动写入
    this.db.prepare(`INSERT OR REPLACE INTO chunks
      (id, text, source, chunk_index, embedding, updated_at)
      VALUES (?, ?, ?, ?, ?, ?)`)
      .run(chunk.id, chunk.text, chunk.source, chunk.index,
           JSON.stringify(embedding), now);

    this.db.prepare(`INSERT OR REPLACE INTO chunks_vec (id, embedding)
      VALUES (?, ?)`)
      .run(chunk.id, Buffer.from(new Float32Array(embedding).buffer));

    this.db.prepare(`INSERT OR REPLACE INTO chunks_fts (id, text, source)
      VALUES (?, ?, ?)`)
      .run(chunk.id, chunk.text, chunk.source);
  }

  addBatch(items: Array<{ chunk: Chunk; embedding: number[] }>): void {
    const tx = this.db.transaction(() => {
      for (const { chunk, embedding } of items) this.add(chunk, embedding);
    });
    tx();  // 事务批量写入，比逐条快很多
  }

  vectorSearch(queryEmbedding: number[], topK: number): Array<{ chunk: StoredChunk; score: number }> {
    const buf = Buffer.from(new Float32Array(queryEmbedding).buffer);
    const rows = this.db.prepare(`
      SELECT v.id, v.distance, c.text, c.source, c.chunk_index, c.embedding
      FROM chunks_vec v
      JOIN chunks c ON c.id = v.id
      WHERE v.embedding MATCH ?
      ORDER BY v.distance
      LIMIT ?
    `).all(buf, topK) as any[];

    return rows.map(r => ({
      chunk: {
        id: r.id, text: r.text, source: r.source,
        index: r.chunk_index,
        tokenEstimate: Math.ceil(r.text.length / 4),
        embedding: JSON.parse(r.embedding),
        addedAt: 0,
      },
      score: 1 - r.distance,  // cosine distance → similarity
    }));
  }

  keywordSearch(query: string, topK: number): Array<{ chunk: StoredChunk; score: number }> {
    const rows = this.db.prepare(`
      SELECT f.id, bm25(chunks_fts) AS rank, c.text, c.source, c.chunk_index, c.embedding
      FROM chunks_fts f
      JOIN chunks c ON c.id = f.id
      WHERE chunks_fts MATCH ?
      ORDER BY rank
      LIMIT ?
    `).all(query, topK) as any[];

    return rows.map(r => ({
      chunk: {
        id: r.id, text: r.text, source: r.source,
        index: r.chunk_index,
        tokenEstimate: Math.ceil(r.text.length / 4),
        embedding: JSON.parse(r.embedding),
        addedAt: 0,
      },
      score: r.rank < 0 ? -r.rank / (1 - r.rank) : 1 / (1 + r.rank),
    }));
  }

  size(): number {
    return (this.db.prepare('SELECT COUNT(*) as n FROM chunks').get() as any).n;
  }

  clear(): void {
    this.db.exec('DELETE FROM chunks; DELETE FROM chunks_vec; DELETE FROM chunks_fts;');
  }

  sources(): string[] {
    return (this.db.prepare('SELECT DISTINCT source FROM chunks').all() as any[]).map(r => r.source);
  }

  // 混合搜索：直接在 SQLite 层完成向量 + 关键词双路检索
  async hybridSearch(
    embedFn: EmbeddingFn,
    query: string,
    topK: number = 5,
  ): Promise<SearchResult[]> {
    const candidateCount = Math.min(topK * 4, this.size());
    if (candidateCount === 0) return [];

    const [queryVec] = await embed(embedFn, [query]);

    // 路径 1: sqlite-vec 向量搜索
    const vectorResults = this.vectorSearch(queryVec, candidateCount);

    // 路径 2: FTS5 关键词搜索
    const keywordResults = this.keywordSearch(query, candidateCount);

    // 归一化 + 加权合并
    const vecScores = normalizeMinMax(vectorResults.map(r => r.score));
    const kwScores = normalizeMinMax(keywordResults.map(r => r.score));

    const candidates = new Map<string, SearchResult>();
    for (let i = 0; i < vectorResults.length; i++) {
      const id = vectorResults[i].chunk.id;
      candidates.set(id, {
        chunk: vectorResults[i].chunk,
        score: vecScores[i] * 0.7,
        vectorScore: vecScores[i],
        keywordScore: 0,
      });
    }
    for (let i = 0; i < keywordResults.length; i++) {
      const id = keywordResults[i].chunk.id;
      const existing = candidates.get(id);
      if (existing) {
        existing.keywordScore = kwScores[i];
        existing.score += kwScores[i] * 0.3;
      } else {
        candidates.set(id, {
          chunk: keywordResults[i].chunk,
          score: kwScores[i] * 0.3,
          vectorScore: 0,
          keywordScore: kwScores[i],
        });
      }
    }

    const sorted = [...candidates.values()]
      .sort((a, b) => b.score - a.score);

    // MMR deduplication
    return mmrSelect(sorted, topK);
  }
}

function normalizeMinMax(scores: number[]): number[] {
  if (scores.length === 0) return [];
  const min = Math.min(...scores);
  const max = Math.max(...scores);
  const range = max - min || 1;
  return scores.map(s => (s - min) / range);
}
```


```typescript
import { embed, type EmbeddingFn } from './embedder.js';
import { mmrSelect, type SearchResult } from './search.js';
```


```typescript
// 替换这两行 import：
// import { VectorStore } from '../rag/store.js';
// import { hybridSearch } from '../rag/search.js';
import { SqliteVectorStore } from '../rag/sqlite-store.js';

// 函数签名也要改：
export function createRagTools(vectorStore: SqliteVectorStore, embedFn: EmbeddingFn): ToolDefinition[] {
  // ...rag_ingest 不变...

  // rag_search 的 execute 里改成：
  const results = await vectorStore.hybridSearch(embedFn, query, top_k || 5);
  // ...其余不变...
}
```


```typescript
// 替换这一行：
// import { VectorStore } from './rag/store.js';
import { SqliteVectorStore } from './rag/sqlite-store.js';

// 替换这一行：
// const vectorStore = new VectorStore();
const vectorStore = new SqliteVectorStore('knowledge.db');
```
