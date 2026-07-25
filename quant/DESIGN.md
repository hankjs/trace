---
name: quant 量化研究决策工作台
description: 面向个人投资者的中文优先 A 股日频研究与手工决策工作台
colors:
  research-cyan: "oklch(0.48 0.105 210)"
  research-cyan-deep: "oklch(0.41 0.11 210)"
  paper-cool: "oklch(0.978 0.005 228)"
  paper-raised: "oklch(0.995 0.003 228)"
  paper-muted: "oklch(0.956 0.006 228)"
  ink-primary: "oklch(0.23 0.018 228)"
  ink-secondary: "oklch(0.45 0.018 228)"
  ink-tertiary: "oklch(0.61 0.014 228)"
  rule: "oklch(0.89 0.009 228)"
  rule-subtle: "oklch(0.935 0.006 228)"
  selected-soft: "oklch(0.91 0.027 210)"
  info-soft: "oklch(0.945 0.025 235)"
  warning-ink: "oklch(0.43 0.095 65)"
  warning-soft: "oklch(0.95 0.035 75)"
  danger-soft: "oklch(0.95 0.035 25)"
  market-up: "oklch(0.55 0.2 25)"
  market-down: "oklch(0.55 0.15 155)"
  on-accent: "oklch(0.985 0.004 210)"
typography:
  headline:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, PingFang SC, Microsoft YaHei, sans-serif"
    fontSize: "1.25rem"
    fontWeight: 600
    lineHeight: 1.4
    letterSpacing: "0"
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, PingFang SC, Microsoft YaHei, sans-serif"
    fontSize: "1rem"
    fontWeight: 600
    lineHeight: 1.5
    letterSpacing: "0"
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, PingFang SC, Microsoft YaHei, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: "0"
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, PingFang SC, Microsoft YaHei, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 500
    lineHeight: 1.35
    letterSpacing: "0"
rounded:
  sm: "4px"
  md: "6px"
  lg: "8px"
  full: "9999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
components:
  button-primary:
    backgroundColor: "{colors.research-cyan}"
    textColor: "{colors.on-accent}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "6px 16px"
    height: "36px"
  button-primary-hover:
    backgroundColor: "{colors.research-cyan-deep}"
    textColor: "{colors.on-accent}"
    rounded: "{rounded.md}"
  button-secondary:
    backgroundColor: "{colors.paper-raised}"
    textColor: "{colors.ink-secondary}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "6px 12px"
    height: "36px"
  input:
    backgroundColor: "{colors.paper-raised}"
    textColor: "{colors.ink-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "6px 8px"
    height: "36px"
  panel:
    backgroundColor: "{colors.paper-raised}"
    textColor: "{colors.ink-primary}"
    rounded: "{rounded.md}"
    padding: "16px"
  nav-active:
    backgroundColor: "{colors.selected-soft}"
    textColor: "{colors.research-cyan}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "6px 10px"
---

# Design System: quant 量化研究决策工作台

## 1. Overview

**Creative North Star: "每日研究台账"**

界面像一份每天都会打开、逐项核对的专业研究台账。它应清晰、克制、可信：中文结论先出现，英文 key 和专业参数按需展开；数据日期、覆盖范围、命中原因和限制始终靠近结果。

这是高信息密度的工作界面，不是金融营销页面。布局依靠明确层级、细分隔线和紧凑表格组织关系，少量青色只标记当前选择和主要动作。功能可以专业，但用户不应被内部字段、原始 JSON 或一次性堆叠的大表单挡在门外。

**Key Characteristics:**

- 中文优先，固定词典是跨页面唯一命名来源。
- 日频研究、模拟回测和手工记账边界清楚可见。
- 表格与分段工作区承载密集信息，教学内容在右侧或移动抽屉渐进展开。
- 数据状态不只用颜色表达，同时提供文字、日期、覆盖数和原因。

## 2. Colors

冷调纸面中性色构成安静的长时间工作环境，低饱和青色作为稀缺的研究焦点；A 股涨红跌绿仅表达市场语义。

### Primary

- **研究青** (`oklch(0.48 0.105 210)`): 主要按钮、当前导航、链接和焦点环。
- **深研究青** (`oklch(0.41 0.11 210)`): 主要动作的悬停状态，不用于大面积背景。

### Secondary

- **上涨红** (`oklch(0.55 0.2 25)`): 正涨跌、风险错误和卖出语义，必须同时配文字或符号。
- **下跌绿** (`oklch(0.55 0.15 155)`): 负涨跌和买入语义，必须同时配文字或符号。
- **校验琥珀** (`oklch(0.43 0.095 65)`): 数据覆盖、口径与注意事项。

### Neutral

- **冷纸底** (`oklch(0.978 0.005 228)`): 页面底色。
- **抬升纸面** (`oklch(0.995 0.003 228)`): 表格、表单和工具面板。
- **静音纸面** (`oklch(0.956 0.006 228)`): 条件行、次级区域和骨架屏。
- **主墨色** (`oklch(0.23 0.018 228)`): 标题与关键数据。
- **次墨色** (`oklch(0.45 0.018 228)`): 正文与非主要操作。
- **注释墨色** (`oklch(0.61 0.014 228)`): 日期、代码、字段标签和补充信息。
- **分隔线** (`oklch(0.89 0.009 228)`): 面板和控件边界；细分隔使用 `oklch(0.935 0.006 228)`。

### Named Rules

**The Research Focus Rule.** 研究青只用于主要动作、当前选择、链接和焦点，不超过单屏视觉面积的约 10%。

**The Market Semantics Rule.** 红绿只表达 A 股涨跌或明确风险状态，任何状态都必须有中文文字、图标或数值符号作为第二通道。

## 3. Typography

**Display Font:** 系统无衬线字体栈，以 PingFang SC / Microsoft YaHei 为中文回退
**Body Font:** 与标题共用系统无衬线字体栈
**Label/Mono Font:** 英文 key 使用浏览器等宽 `code` 样式，其余标签保持系统无衬线

**Character:** 单一字体家族让研究工具保持熟悉和稳定。层级来自字号、字重和留白，不使用展示型字体或紧缩字距制造戏剧性。

### Hierarchy

- **Headline** (600, 20px, 1.4): 页面唯一主标题。
- **Title** (600, 16px, 1.5): 工作区、表格或结果段标题。
- **Body** (400, 14px, 1.5): 表格、表单、说明与按钮；长说明使用 24px 行高并限制在约 72ch。
- **Label** (500, 12px, 1.35): 字段名、日期、单位、代码和辅助状态。

### Named Rules

**The Fixed Scale Rule.** 产品字号固定为 12、14、16、18、20px 的紧凑层级，不按视口流体缩放，字距始终为 0。

## 4. Elevation

系统默认扁平，以冷纸底、抬升纸面和 1px 分隔线建立层次。阴影只用于真正离开文档流的搜索下拉和移动抽屉，不把普通页面区段做成漂浮卡片。

### Shadow Vocabulary

- **浮层阴影** (`0 8px 24px oklch(0.25 0.02 228 / 0.12)`): 搜索建议和需要遮挡下层内容的临时浮层。
- **抽屉阴影** (`-12px 0 40px oklch(0.2 0.02 228 / 0.18)`): 移动研究助手从右侧打开时建立模态层次。

### Named Rules

**The Flat Research Rule.** 页面区段与表格在静止状态依靠背景和边框分层；只有浮层、抽屉和明确覆盖关系可以使用阴影。

## 5. Components

### Buttons

- **Shape:** 紧凑矩形，6px 圆角，高度 36px；纯图标按钮固定为 36 × 36px。
- **Primary:** 研究青背景、近白文字、水平内边距 16px，仅用于当前流程的主要提交动作。
- **Hover / Focus:** 悬停变为深研究青；键盘焦点统一使用 2px 研究青外轮廓和 2px 偏移。
- **Secondary / Ghost:** 抬升纸面或透明背景，1px 分隔线，悬停使用静音纸面；熟悉的工具动作优先使用 Lucide 图标和 tooltip。

### Chips

- **Style:** 4px 圆角、小号文字、柔和语义底色；用于命中原因、状态和非主要分类。
- **State:** 选中态使用柔和青底与研究青文字，未选中态保持透明或抬升纸面并使用分隔线。

### Cards / Containers

- **Corner Style:** 工具面板和表格使用 6px，独立摘要项最多 8px。
- **Background:** 抬升纸面；条件行等内部编辑区域使用静音纸面，但不再套一层卡片边框。
- **Shadow Strategy:** 默认无阴影，遵守 Flat Research Rule。
- **Border:** 1px 分隔线；表格内部使用更浅的细分隔线。
- **Internal Padding:** 紧凑区域 12px，常规工具面板 16px，页面主要区段间距 20–24px。

### Inputs / Fields

- **Style:** 抬升纸面、1px 分隔线、6px 圆角、高度约 36px；可见中文标签放在控件上方。
- **Focus:** 2px 研究青外轮廓，不依赖只改变边框颜色。
- **Error / Disabled:** 错误同时提供中文说明和柔和危险底色；禁用态降低透明度并保留原因提示。

### Navigation

顶部导航在宽屏保持单行，窄屏允许水平滚动。当前项使用柔和青底、研究青文字和中等字重；工作区内二级视图使用下划线 tab。移动研究助手是带焦点圈定、Escape 关闭和焦点恢复的右侧模态抽屉。

### Structured Filter Builder

条件以可独立启停的行组织，字段、关系、数值和单独命中数保持同一阅读顺序。AND / OR 使用分段控制；结果必须同时展示研究范围、数据覆盖、命中原因、估值日期和财报期。缺少或过期的数据按缺失处理，不得默认为通过。

## 6. Do's and Don'ts

### Do:

- **Do** 让所有股票先显示中文名称，代码以 12px 注释文字紧邻展示。
- **Do** 从固定中英文字典读取指标、策略、信号和回测指标名称，英文 key 仅作专业补充。
- **Do** 在筛选、信号和回测结果附近展示数据日期、覆盖范围、命中原因与限制。
- **Do** 使用 1px 分隔线、6px 圆角、12–16px 内边距和固定字号保持高密度但可扫读。
- **Do** 为加载、空结果、缺失数据、错误、禁用和键盘焦点提供完整状态。
- **Do** 明确写出日频研究、模拟回测与外部手工交易的产品边界。

### Don't:

- **Don't** 直接暴露英文内部字段、原始 JSON 原因或没有中文解释的复杂参数。
- **Don't** 把所有能力堆进同一张表单；用工作区、条件组和渐进展开组织任务。
- **Don't** 使用营销型金融视觉、夸张收益表达、交易诱导，或任何自动和半自动下单暗示。
- **Don't** 用装饰性卡片、大面积颜色、渐变文字、玻璃效果或装饰动效掩盖信息关系。
- **Don't** 在卡片中再嵌套卡片，也不要给普通页面区段添加漂浮阴影。
- **Don't** 只用红绿颜色表达状态，或让鼠标成为展开、关闭和选择的唯一方式。
- **Don't** 使用超过 1px 的彩色侧边条作为提醒或卡片装饰。
