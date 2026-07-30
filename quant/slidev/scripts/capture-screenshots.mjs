/**
 * 线上 quant 全页截图（凭据仅通过环境变量，不入库）
 *
 *   QUANT_BASE_URL=http://host:8100 \
 *   QUANT_USER=admin QUANT_PASS='...' \
 *   node scripts/capture-screenshots.mjs
 */
import { chromium } from 'playwright'
import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const OUT_DIR = path.resolve(__dirname, '../public/screenshots')
const BASE = (process.env.QUANT_BASE_URL || 'http://111.170.174.167:8100').replace(/\/$/, '')
const USER = process.env.QUANT_USER || ''
const PASS = process.env.QUANT_PASS || ''

if (!USER || !PASS) {
  console.error('需要环境变量 QUANT_USER / QUANT_PASS')
  process.exit(1)
}

const PAGES = [
  { name: '01-login', path: '/login', public: true, wait: 800 },
  { name: '02-dashboard', path: '/', wait: 2000 },
  { name: '03-watchlist', path: '/watchlist', wait: 1500 },
  { name: '04-selection', path: '/selection', wait: 2000 },
  { name: '05-pools', path: '/pools', wait: 1500 },
  { name: '06-signals', path: '/signals', wait: 2000 },
  { name: '07-strategies', path: '/strategies/manage', wait: 2000 },
  { name: '08-backtest', path: '/strategies/backtest', wait: 2000 },
  { name: '09-experiments', path: '/strategies/experiments', wait: 1500 },
  { name: '10-leaderboard', path: '/strategies/leaderboard', wait: 1500 },
  { name: '11-portfolio', path: '/portfolio', wait: 1500 },
  { name: '12-catalog', path: '/catalog', wait: 1500 },
  { name: '13-tasks', path: '/tasks', wait: 1500 },
  { name: '14-settings', path: '/settings', wait: 1200 },
  { name: '15-admin-jobs', path: '/admin/jobs', wait: 2000 },
]

async function loginViaApi() {
  const res = await fetch(`${BASE}/api/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: USER, password: PASS }),
  })
  if (!res.ok) {
    throw new Error(`登录失败 HTTP ${res.status}: ${await res.text()}`)
  }
  return res.json()
}

async function injectAuth(page, auth) {
  await page.goto(`${BASE}/login`, { waitUntil: 'domcontentloaded' })
  await page.evaluate(
    ({ token, username, canAdmin }) => {
      localStorage.setItem('quant_token', token)
      localStorage.setItem('quant_username', username)
      localStorage.setItem('quant_can_admin', String(canAdmin))
    },
    {
      token: auth.token,
      username: auth.username,
      canAdmin: Boolean(auth.can_admin),
    },
  )
}

async function settle(page, ms) {
  await page.waitForTimeout(ms)
  // 关掉可能的 onboarding / tour 遮罩
  for (const sel of [
    'button:has-text("跳过")',
    'button:has-text("知道了")',
    'button:has-text("完成")',
    'button:has-text("关闭")',
    '[data-tour-skip]',
    '.tour-close',
  ]) {
    try {
      const btn = page.locator(sel).first()
      if (await btn.isVisible({ timeout: 300 })) await btn.click({ timeout: 500 })
    } catch {
      /* ignore */
    }
  }
  await page.waitForTimeout(400)
}

async function shot(page, name) {
  const file = path.join(OUT_DIR, `${name}.png`)
  await page.screenshot({ path: file, fullPage: true })
  console.log('saved', path.basename(file))
  return file
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true })
  console.log('login', BASE, USER)
  const auth = await loginViaApi()
  console.log('login ok, can_admin=', auth.can_admin)

  const browser = await chromium.launch({
    headless: true,
    args: ['--disable-dev-shm-usage'],
  })
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1.5,
    locale: 'zh-CN',
  })
  const page = await context.newPage()
  page.setDefaultTimeout(30000)

  // 登录页（未注入 token）
  await page.goto(`${BASE}/login`, { waitUntil: 'networkidle' })
  await settle(page, 800)
  await shot(page, '01-login')

  await injectAuth(page, auth)

  let stockCode = null
  for (const item of PAGES) {
    if (item.public) continue
    await page.goto(`${BASE}${item.path}`, { waitUntil: 'networkidle' })
    await settle(page, item.wait || 1500)
    // 若被踢回登录，重试注入
    if (page.url().includes('/login')) {
      await injectAuth(page, auth)
      await page.goto(`${BASE}${item.path}`, { waitUntil: 'networkidle' })
      await settle(page, item.wait || 1500)
    }
    await shot(page, item.name)

    // 从 dashboard / selection 尝试抓一个股票代码
    if (!stockCode && (item.name === '02-dashboard' || item.name === '04-selection')) {
      stockCode = await page.evaluate(() => {
        const text = document.body.innerText || ''
        const m = text.match(/\b(?:sh|sz|bj)\.\d{6}\b/i)
        return m ? m[0] : null
      })
    }
  }

  if (!stockCode) {
    // API 兜底：拉候选或股票列表
    try {
      const r = await fetch(`${BASE}/api/market/stocks?limit=1`, {
        headers: { Authorization: `Bearer ${auth.token}` },
      })
      if (r.ok) {
        const data = await r.json()
        const row = Array.isArray(data) ? data[0] : data?.items?.[0] || data?.stocks?.[0]
        stockCode = row?.code || row?.symbol || null
      }
    } catch {
      /* ignore */
    }
  }
  if (!stockCode) stockCode = 'sh.600519'

  await page.goto(`${BASE}/stock/${encodeURIComponent(stockCode)}`, {
    waitUntil: 'networkidle',
  })
  await settle(page, 2500)
  await shot(page, '16-stock-detail')

  // 策略页尝试点开第一条以便看到 Spec 编辑区
  await page.goto(`${BASE}/strategies/manage`, { waitUntil: 'networkidle' })
  await settle(page, 1500)
  try {
    const row = page.locator('table tbody tr, [class*="strategy"] button, a').filter({
      hasText: /策略|均线|突破|动量/,
    }).first()
    if (await row.isVisible({ timeout: 1500 })) {
      await row.click({ timeout: 2000 })
      await settle(page, 2000)
      await shot(page, '17-strategy-detail')
    }
  } catch (e) {
    console.warn('strategy detail skip:', e.message)
  }

  // 选股工作区若有 tab，截筛选态
  await page.goto(`${BASE}/selection`, { waitUntil: 'networkidle' })
  await settle(page, 1500)
  for (const label of ['筛选', '条件', 'Screener', '候选']) {
    try {
      const tab = page.getByRole('button', { name: new RegExp(label) }).first()
      if (await tab.isVisible({ timeout: 500 })) {
        await tab.click()
        await settle(page, 1500)
        await shot(page, '18-selection-screener')
        break
      }
      const tab2 = page.getByText(label, { exact: false }).first()
      if (await tab2.isVisible({ timeout: 500 })) {
        await tab2.click()
        await settle(page, 1500)
        await shot(page, '18-selection-screener')
        break
      }
    } catch {
      /* try next */
    }
  }

  await writeFile(
    path.join(OUT_DIR, 'manifest.json'),
    JSON.stringify(
      {
        base: BASE,
        captured_at: new Date().toISOString(),
        stock: stockCode,
        pages: PAGES.map((p) => p.name).concat([
          '16-stock-detail',
          '17-strategy-detail',
          '18-selection-screener',
        ]),
      },
      null,
      2,
    ),
  )

  await browser.close()
  console.log('done →', OUT_DIR)
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
