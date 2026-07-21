# Skills——给 Agent 注入领域知识

课程
Super Agent 实战课
Skills——给 Agent 注入领域知识
Skills——给 Agent 注入领域知识

约 21 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

你让 Agent 做代码审查，它也能做，但每次给出来的东西不太一样——该查什么、按什么优先级、输出什么格式，全看它当时的"心情"。说实话就是缺一份 SOP，有了标准流程它就稳定了。知识体系课里讲过 Skill 这个概念，这一节我们把它落地——给 Agent 装上 Skill 加载能力，写一个 code-review skill 实际跑一把。

先装依赖：

bash
运行
复制
pnpm install

Skill 不是 Tool

这个区分很重要，所以一开始就说清楚。

Tool 是一个可执行的函数——read_file、grep、bash。Agent 调用它，拿到返回值，然后继续推理。Tool 的本质是"一个原子操作"。你想想，read_file 不会告诉 Agent "你应该先读哪个文件"，它只负责"你让我读什么我就读什么"。

Skill 是一份知识文档——用 Markdown 写的行为指导。它不注册到 tools 列表里，而是注入到 system prompt 里。Agent 读了这份文档之后，就知道"做代码审查时应该先看 diff，再逐文件检查 SOLID，再扫安全风险，最后按 P0-P3 分级输出"。

换个说法——Tool 是它手里的锤子和扳手，Skill 是它脑子里的操作手册。有工具但没有方法论，等于有一堆零件不知道怎么组装。

Claude Code 的 skills 系统就是这个设计——.claude/skills/skill-name/SKILL.md，用户输入 /skill-name 触发。知识体系课里也专门讲过 Skill 和 Tool 的区别。

实现 SkillLoader

Skill 的存储很简单——一个目录下放一个 SKILL.md 文件：

text
复制
.skills/
  code-review/
    SKILL.md     ← 代码审查的 SOP
  research/
    SKILL.md     ← 技术调研的 SOP


SKILL.md 用 YAML frontmatter 记录名称和描述，正文是 Markdown 格式的行为指导：

markdown
复制
---
name: code-review
description: "以高级工程师视角审查代码变更"
---

# Code Review

## 审查流程

**1) 收集变更范围**
用 list_directory 和 glob 扫描项目结构...

**2) 架构和设计审查**
关注 SRP、OCP、DIP 等问题...


来，新建 src/skills/loader.ts：

src/skills/loader.ts
应用
复制
import fs from 'node:fs';
import path from 'node:path';

export interface SkillDefinition {
  name: string;
  description: string;
  whenToUse?: string;
  content: string;
  dirPath: string;
}

const SKILLS_DIR = '.skills';
const SKILL_FILE = 'SKILL.md';

export class SkillLoader {
  private readonly baseDir: string;
  private skills = new Map<string, SkillDefinition>();

  constructor(baseDir = '.') {
    this.baseDir = baseDir;
  }

  load(): SkillDefinition[] {
    this.skills.clear();
    const skillsDir = path.join(this.baseDir, SKILLS_DIR);
    if (!fs.existsSync(skillsDir)) return [];

    const entries = fs.readdirSync(skillsDir, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      const skillFile = path.join(skillsDir, entry.name, SKILL_FILE);
      if (!fs.existsSync(skillFile)) continue;

      const raw = fs.readFileSync(skillFile, 'utf-8');
      const parsed = this.parseFrontmatter(raw);
      if (!parsed) continue;

      this.skills.set(entry.name, {
        name: entry.name,
        description: parsed.description,
        content: parsed.content,
        dirPath: path.join(skillsDir, entry.name),
      });
    }
    return this.list();
  }

  list(): SkillDefinition[] {
    return Array.from(this.skills.values());
  }

  get(name: string): SkillDefinition | undefined {
    return this.skills.get(name);
  }

  buildPromptSection(activeSkills: Set<string>): string | null {
    if (this.skills.size === 0) return null;
    const lines: string[] = [];

    for (const name of activeSkills) {
      const skill = this.skills.get(name);
      if (!skill) continue;
      lines.push(`[激活的 Skill: ${skill.name}]`);
      lines.push(skill.content);
      lines.push('');
    }

    const available = this.list()
      .filter(s => !activeSkills.has(s.name))
      .map(s => `  /${s.name} — ${s.description}`);
    if (available.length > 0) {
      lines.push('可用的 Skills（输入 /skill load <name> 激活）：');
      lines.push(...available);
    }
    return lines.length > 0 ? lines.join('\n') : null;
  }

  private parseFrontmatter(raw: string): { description: string; content: string } | null {
    const match = raw.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/);
    if (!match) return { description: '', content: raw };
    const meta: Record<string, string> = {};
    for (const line of match[1].split('\n')) {
      const idx = line.indexOf(':');
      if (idx > 0) {
        const key = line.slice(0, idx).trim();
        let value = line.slice(idx + 1).trim();
        if (value.startsWith('"') && value.endsWith('"')) value = value.slice(1, -1);
        meta[key] = value;
      }
    }
    return { description: meta.description || '', content: match[2].trim() };
  }
}


load() 做的事情是这样的：扫描 .skills/ 目录，每个子目录找 SKILL.md，解析 frontmatter 拿到描述，正文存起来。frontmatter 的解析我们手写了个简易版——按行拆、按冒号分 key/value。生产环境如果需要支持多行值、嵌套对象这些复杂语法，可以换成 gray-matter，社区标准的 frontmatter 解析库，这个解析部分不是重点，我们就不过多展开了。

buildPromptSection 是 Prompt 的注入点，也是整个 skill 系统最关键的设计——渐进式加载。

知识体系课里详细讲过这个概念，Claude Code 的实现分三层：

Level 1 启动时只加载 frontmatter（name + description + when_to_use，每个 skill 大概 100 token）；
Level 2 用户激活后才加载完整内容；
Level 3 skill 目录下的参考文件按需用 Read 工具读取。

我们这里做了前两层：未激活的 skill 只注入名称和描述（让模型知道还有哪些可以用），激活了才把全文塞进 system prompt。这样 100 个 skill 初始也就 10K token，不会把上下文撑爆。Level 3 不需要额外实现——skill 目录下放的参考文件就是普通文件，Agent 用 read_file 随时能读，天然就是按需加载。

实际上 SkillLoader 已经支持了 when_to_use 字段——frontmatter 里写上 when_to_use，parseFrontmatter 会把它解析出来存到 SkillDefinition.whenToUse。在 buildPromptSection 列出未激活 skill 时，如果有 whenToUse 就会追加一个 (适用场景: ...) 的提示，模型看到后能更准确地判断什么时候该建议用户激活。比如 code-review 的 when_to_use 可以写：

当用户要求审查代码、review PR、或检查代码质量时


模型看到这个提示后，在用户说"帮我看看这段代码"的时候就会主动建议/code-review。

写一个 code-review skill

有了 SkillLoader，接下来就是写内容了。一个好的 skill 应该回答三个问题：做什么（审查代码变更）、怎么做（按什么步骤和优先级）、输出什么（报告格式和分级标准）。

参考 sanyuan0704/sanyuan-skills 的 code review skill，我们写一个简化版。完整版包含更多的 checklist（安全、性能、竞态条件等），这里抽取核心骨架：

新建 .skills/code-review/SKILL.md：

.skills/code-review/skill.md
应用
复制
---
name: code-review
description: "以高级工程师视角审查项目代码，检测 SOLID 违规、安全风险、性能隐患"
when_to_use: "当用户要求审查代码、review 代码、或检查代码质量时"
---

# Code Review

## 审查流程

**1) 确定审查范围**

先问用户要审查哪部分代码。如果用户没指定，用 `list_directory` 看一下目录结构，让用户选模块。不要一次审查整个项目——聚焦到具体目录。

**2) 逐文件阅读和审查**

用 `read_file` 读取目标模块的源码，关注：
- **SRP**：一个模块是不是干了太多不相关的事
- **OCP**：新功能是靠修改已有代码实现的，还是通过扩展点
- **DIP**：高层逻辑是不是直接依赖了低层实现

**3) 安全扫描**

检查：注入漏洞、认证/授权缺口、密钥泄漏、竞态条件。

**4) 代码质量**

检查：错误处理、N+1 查询、边界条件、async 异常。

## 输出格式

按 P0（必须修复）、P1（建议修复）、P2（可以改进）分级输出。
每个发现标注文件:行号和具体建议。
默认只输出审查结果，不直接改代码——除非用户明确要求。


这份 skill 做了两件事。一是定义了审查流程——先扫项目结构，再逐文件审查架构设计，再扫安全风险，最后看代码质量。二是规范了输出格式——按 P0/P1/P2 分级，每个发现标注文件和行号。Agent 拿到这份 SOP，就不再是"随便看看"了，而是有章法、有标准的专业审查。

激活和使用

集成到 index.ts 只需要三步：创建 SkillLoader、调 load() 扫描可用的 skill、把 buildPromptSection 接到 PromptBuilder 的 pipe 上（跟 memory、RAG 一样的套路——每轮对话前动态构建 system prompt）。

/skill 命令负责管理：/skill list 列出所有 skill、/skill load code-review 激活一个、/skill unload code-review 卸载。也可以直接输入 /code-review 查看 rag 的实现——激活 skill 的同时把 skill 内容作为 user message 注入，Agent 拿到 SOP 后立刻按流程开始执行。这个快捷方式很关键，因为大多数时候你只是想用一下某个 skill，不需要先 load 再打字描述需求。

bash
运行
复制
pnpm start


试试 /skill 看有什么可用的：

text
复制
You: /skill

[skills] 共 1 个可用：
  /code-review — 以高级工程师视角审查代码变更，检测 SOLID 违规、安全风险、性能隐患


试试输入 /code-review 查看 rag 的实现——Agent 会按 SKILL.md 里的四步流程执行：先确认范围是 src/rag/，再逐文件读代码审查，最后按 P0-P2 格式输出报告。

你对比一下有 skill 和没 skill 的区别：没 skill 的时候你说"帮我 review 一下代码"，Agent 可能不知道按照什么规则去 review，输出质量很不稳定。有了 code-review skill 之后，它会严格按照四步流程走，每个发现标注到具体的文件和行号，按严重程度分级。输出的一致性和专业度完全不是一个级别。

你完全可以自己写更多 skill——比如 research（调研模式：搜索、整理、输出结构化报告）、debug（故障排查：收集日志、定位根因、给出修复方案）。一个 .skills/xxx/SKILL.md 文件就是一个 skill，不需要改任何代码。

Skill 激活后发生了什么

好，功能实现出来了。有几个重点在这里再跟大家回顾一下。

激活一个 skill 之后，buildPromptSection 把它的完整内容注入到 system prompt 里。模型每一轮推理都能看到这份 SOP——它的行为就会跟着变。

这里有个容易忽略的设计决策：未激活的 skill 不注入内容，只注入名称和描述。这是为了控制 token 开销。一个 code-review skill 可能有几百字的内容，如果你还有 research、teach 等等几个 skill，全塞进去 system prompt 会膨胀得很快。

还有一个点是多个 skill 可以同时激活。你可以同时 load code-review 和 research，Agent 会把两份 SOP 都记在脑子里。

其实你仔细想想，CLAUDE.md 本身就是一种"永久激活的 skill"——它也是往 system prompt 里注入一份行为指导。区别在于 CLAUDE.md 是全局的、每次对话都加载，而 skill 是按需激活的、用完可以卸载。

写在最后

Skill 这个概念看着简单——说白了就是往 prompt 里塞一份 Markdown 文档嘛。但它背后解决了一个实际的问题：让 Agent 在特定领域从"通才"变成"专家"。

Tool 给了 Agent 一双手，让它能操作代码、文件、数据库。Skill 给了它一套方法论，让它知道在什么场景下该怎么用这双手。

而且 skill 最大的好处是零代码扩展——不需要改 Agent 核心代码，写一个 .md 文件就能增加一种专业能力。这也是为什么 Skills 生态在 2026 年会井喷式爆发——大家可以以非常低的成本共享自己的 SOP，让其他人拿来就用。

下一节我们继续往上走——Plugin 架构。Skill 是往 prompt 里注入知识，Plugin 是往运行时里注入代码——新 Tool、新 Channel、新 Cron Job，全都可以通过 Plugin 动态加载。我们下一节，精彩继续。

参考资料
Claude Code Skills 文档 — 官方 skill 系统设计
sanyuan0704/sanyuan-skills — code-review-expert 是本节参考的一个 skills
上一篇
记忆会变坏——给 Agent 的记忆库做体检
下一篇 · 第五章：Skills + Plugins + Channel
Plugin 架构——让别人给你的 Agent 写功能
编辑器


---
## 代码块


```bash
pnpm install
```


```text
.skills/
  code-review/
    SKILL.md     ← 代码审查的 SOP
  research/
    SKILL.md     ← 技术调研的 SOP
```


```markdown
---
name: code-review
description: "以高级工程师视角审查代码变更"
---

# Code Review

## 审查流程

**1) 收集变更范围**
用 list_directory 和 glob 扫描项目结构...

**2) 架构和设计审查**
关注 SRP、OCP、DIP 等问题...
```


```ts
import fs from 'node:fs';
import path from 'node:path';

export interface SkillDefinition {
  name: string;
  description: string;
  whenToUse?: string;
  content: string;
  dirPath: string;
}

const SKILLS_DIR = '.skills';
const SKILL_FILE = 'SKILL.md';

export class SkillLoader {
  private readonly baseDir: string;
  private skills = new Map<string, SkillDefinition>();

  constructor(baseDir = '.') {
    this.baseDir = baseDir;
  }

  load(): SkillDefinition[] {
    this.skills.clear();
    const skillsDir = path.join(this.baseDir, SKILLS_DIR);
    if (!fs.existsSync(skillsDir)) return [];

    const entries = fs.readdirSync(skillsDir, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      const skillFile = path.join(skillsDir, entry.name, SKILL_FILE);
      if (!fs.existsSync(skillFile)) continue;

      const raw = fs.readFileSync(skillFile, 'utf-8');
      const parsed = this.parseFrontmatter(raw);
      if (!parsed) continue;

      this.skills.set(entry.name, {
        name: entry.name,
        description: parsed.description,
        content: parsed.content,
        dirPath: path.join(skillsDir, entry.name),
      });
    }
    return this.list();
  }

  list(): SkillDefinition[] {
    return Array.from(this.skills.values());
  }

  get(name: string): SkillDefinition | undefined {
    return this.skills.get(name);
  }

  buildPromptSection(activeSkills: Set<string>): string | null {
    if (this.skills.size === 0) return null;
    const lines: string[] = [];

    for (const name of activeSkills) {
      const skill = this.skills.get(name);
      if (!skill) continue;
      lines.push(`[激活的 Skill: ${skill.name}]`);
      lines.push(skill.content);
      lines.push('');
    }

    const available = this.list()
      .filter(s => !activeSkills.has(s.name))
      .map(s => `  /${s.name} — ${s.description}`);
    if (available.length > 0) {
      lines.push('可用的 Skills（输入 /skill load <name> 激活）：');
      lines.push(...available);
    }
    return lines.length > 0 ? lines.join('\n') : null;
  }

  private parseFrontmatter(raw: string): { description: string; content: string } | null {
    const match = raw.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/);
    if (!match) return { description: '', content: raw };
    const meta: Record<string, string> = {};
    for (const line of match[1].split('\n')) {
      const idx = line.indexOf(':');
      if (idx > 0) {
        const key = line.slice(0, idx).trim();
        let value = line.slice(idx + 1).trim();
        if (value.startsWith('"') && value.endsWith('"')) value = value.slice(1, -1);
        meta[key] = value;
      }
    }
    return { description: meta.description || '', content: match[2].trim() };
  }
}
```


```
当用户要求审查代码、review PR、或检查代码质量时
```


```markdown
---
name: code-review
description: "以高级工程师视角审查项目代码，检测 SOLID 违规、安全风险、性能隐患"
when_to_use: "当用户要求审查代码、review 代码、或检查代码质量时"
---

# Code Review

## 审查流程

**1) 确定审查范围**

先问用户要审查哪部分代码。如果用户没指定，用 `list_directory` 看一下目录结构，让用户选模块。不要一次审查整个项目——聚焦到具体目录。

**2) 逐文件阅读和审查**

用 `read_file` 读取目标模块的源码，关注：
- **SRP**：一个模块是不是干了太多不相关的事
- **OCP**：新功能是靠修改已有代码实现的，还是通过扩展点
- **DIP**：高层逻辑是不是直接依赖了低层实现

**3) 安全扫描**

检查：注入漏洞、认证/授权缺口、密钥泄漏、竞态条件。

**4) 代码质量**

检查：错误处理、N+1 查询、边界条件、async 异常。

## 输出格式

按 P0（必须修复）、P1（建议修复）、P2（可以改进）分级输出。
每个发现标注文件:行号和具体建议。
默认只输出审查结果，不直接改代码——除非用户明确要求。
```


```bash
pnpm start
```


```text
You: /skill

[skills] 共 1 个可用：
  /code-review — 以高级工程师视角审查代码变更，检测 SOLID 违规、安全风险、性能隐患
```
