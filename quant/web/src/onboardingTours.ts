import type { TourStep } from './tour'

/** 新手任务对应的页面聚焦引导,锚点为各视图上的 data-tour 属性 */
export const ONBOARDING_TOURS: Record<string, TourStep[]> = {
  visit_dashboard: [
    {
      target: 'dashboard-data-trust',
      title: '数据信任条',
      content: '先看这里确认最新日线日期与数据覆盖率。数据不新鲜或覆盖不足时,下面的结论都要打折。',
    },
    {
      target: 'dashboard-watch',
      title: '自选行情面板',
      content: '你加入自选的股票最新价显示在这里,盘中快照也按自选名单采集。',
    },
    {
      target: 'dashboard-picks',
      title: '系统候选面板',
      content: '每日评分流程给出的候选摘要,点「完整列表」可进入选股中心查看全部。',
    },
    {
      target: 'dashboard-signal',
      title: '策略提示面板',
      content: '策略状态变化的提醒,只是研究信息,不是买卖指令。',
    },
  ],
  add_watch: [
    {
      target: 'watchlist-add',
      title: '添加自选表单',
      content: '输入股票名称或代码搜索,从下拉候选中选中一只股票。',
    },
    {
      target: 'watchlist-add-button',
      title: '加入自选',
      content: '选中股票后,点击这个「加入自选」按钮把它加入名单。',
      advanceOn: 'target',
    },
    {
      target: 'watchlist-list',
      title: '我的自选列表',
      content: '已加入的股票显示在这里,不需要时可以随时移出。',
    },
  ],
  view_picks: [
    {
      target: 'selection-tabs',
      title: '工作区切换',
      content: '选股中心有两个工作区:「系统候选」是固定评分流程的结果,「组合筛选」由你自己设置条件。',
    },
    {
      target: 'picks-form',
      title: '日期查询表单',
      content: '切换研究日期,可以回看历史某一天的候选名单。',
    },
    {
      target: 'picks-result',
      title: 'Top 30 结果表',
      content: '按评分排序的候选列表,展开每一行可以查看各项指标的明细数值。',
    },
  ],
  run_screener: [
    {
      target: 'screener-pool',
      title: '研究范围(股票池)',
      content: '筛选只在所选股票池内进行,先确认研究范围。',
    },
    {
      target: 'screener-builder',
      title: '条件构建区',
      content: '逐条设置字段、判断关系和阈值,可以组合多个条件组。',
    },
    {
      target: 'screener-run',
      title: '开始筛选',
      content: '条件设好后,点击「开始筛选」执行组合筛选。',
      advanceOn: 'target',
    },
    {
      target: 'screener-result',
      title: '筛选结果区',
      content: '命中股票列在这里,可以查看每条条件单独的命中数。',
    },
  ],
  view_signals: [
    {
      target: 'signals-form',
      title: '过滤表单',
      content: '按日期、股票和提示类型过滤信号。',
    },
    {
      target: 'signals-strategy',
      title: '策略过滤',
      content: '只看某一个策略产生的提示,便于跟踪单个策略的状态变化。',
    },
    {
      target: 'signals-result',
      title: '信号结果表',
      content: '展开每一行可以阅读产生这条提示的具体原因。信号是提醒,不是买卖指令。',
    },
  ],
  duplicate_strategy: [
    {
      target: 'strategies-list',
      title: '策略列表',
      content: '带锁的是公共策略,只读;先在列表中选中一个公共策略。',
    },
    {
      target: 'strategies-detail',
      title: '策略详情区',
      content: '选中策略后在这里查看规则、参数和证据状态。',
    },
    {
      target: 'strategies-save-as',
      title: '另存为我的策略',
      content: '点击「另存为」复制成自己的策略,之后就能调参并用于回测。',
    },
  ],
  run_backtest: [
    {
      target: 'backtest-strategy',
      title: '选择策略',
      content: '选择要验证的策略,回测会固化当前完整规格。',
    },
    {
      target: 'backtest-dates',
      title: '日期区间',
      content: '设置回测的起止日期,不同市场阶段的表现可能差异很大。',
    },
    {
      target: 'backtest-scope',
      title: '选股方式',
      content: '可以手动选几只股票,或按股票池在区间内逐日解析成分。',
    },
    {
      target: 'backtest-cost',
      title: '费用假设',
      content: '展开可调整佣金、印花税和滑点,费用会明显影响模拟结果。',
    },
    {
      target: 'backtest-run',
      title: '运行回测',
      content: '点击「运行回测」提交模拟,结果用于验证规则,不是收益承诺。',
    },
  ],
  add_trade: [
    {
      target: 'portfolio-summary',
      title: '持仓汇总卡',
      content: '按最新显示价格估算的总市值与浮动/已实现盈亏。',
    },
    {
      target: 'portfolio-form',
      title: '手工记账表单',
      content: '记录已在外部交易软件中完成的真实成交,系统不会提交订单。',
    },
    {
      target: 'portfolio-trades',
      title: '成交记录表',
      content: '所有手工记录列在这里,记错的可以删除后重新录入。',
    },
  ],
}
