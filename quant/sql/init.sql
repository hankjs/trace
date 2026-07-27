-- quant 全新数据库初始化脚本（MySQL 5.7+/8.0）
--
-- 仅用于空数据库。已有数据库必须执行 `uv run alembic upgrade head`，不要用本
-- 脚本覆盖或补建，否则 Alembic 无法可靠判断已经执行过哪些数据迁移。
-- 本脚本只管理 quant_* 表；与主服务共享、由 app/auth.py 只读访问的 users 表
-- 不属于 quant schema，不在这里创建。
--
-- Schema revision: 0013_research_plan

SET NAMES utf8mb4;

CREATE TABLE `alembic_version` (
  `version_num` VARCHAR(32) NOT NULL,
  PRIMARY KEY (`version_num`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_adjust_factor` (
  `code` VARCHAR(16) NOT NULL,
  `divid_operate_date` DATE NOT NULL,
  `fore_factor` DECIMAL(16, 6) NOT NULL,
  `back_factor` DECIMAL(16, 6) DEFAULT NULL,
  `source` VARCHAR(16) NOT NULL DEFAULT 'baostock',
  PRIMARY KEY (`code`, `divid_operate_date`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_daily_bar` (
  `code` VARCHAR(16) NOT NULL,
  `date` DATE NOT NULL,
  `open` DECIMAL(12, 4) NOT NULL,
  `high` DECIMAL(12, 4) NOT NULL,
  `low` DECIMAL(12, 4) NOT NULL,
  `close` DECIMAL(12, 4) NOT NULL,
  `raw_close` DECIMAL(12, 4) DEFAULT NULL,
  `volume` DECIMAL(20, 2) NOT NULL,
  `amount` DECIMAL(20, 2) NOT NULL,
  `is_st` TINYINT(1) DEFAULT NULL,
  PRIMARY KEY (`code`, `date`),
  KEY `ix_quant_daily_bar_date` (`date`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_factor_daily` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `code` VARCHAR(16) NOT NULL,
  `date` DATE NOT NULL,
  `mom20` FLOAT DEFAULT NULL,
  `mom60` FLOAT DEFAULT NULL,
  `rsi14` FLOAT DEFAULT NULL,
  `atr_pct` FLOAT DEFAULT NULL,
  `vol_ratio5` FLOAT DEFAULT NULL,
  `ma20_slope` FLOAT DEFAULT NULL,
  `amount_avg20` FLOAT DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_factor_code_date` (`code`, `date`),
  KEY `ix_quant_factor_daily_date` (`date`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_fundamental_snapshot` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `code` VARCHAR(16) NOT NULL,
  `data_date` DATE NOT NULL,
  `report_period` DATE NOT NULL,
  `available_date` DATE NOT NULL,
  `source` VARCHAR(96) NOT NULL,
  `roe` FLOAT DEFAULT NULL,
  `revenue_yoy` FLOAT DEFAULT NULL,
  `profit_yoy` FLOAT DEFAULT NULL,
  `gross_margin` FLOAT DEFAULT NULL,
  `net_margin` FLOAT DEFAULT NULL,
  `debt_ratio` FLOAT DEFAULT NULL,
  `cashflow_ratio` FLOAT DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_fundamental_code_period_available`
    (`code`, `report_period`, `available_date`),
  KEY `ix_quant_fundamental_snapshot_available_date` (`available_date`),
  KEY `ix_quant_fundamental_snapshot_code` (`code`),
  KEY `ix_quant_fundamental_snapshot_data_date` (`data_date`),
  KEY `ix_quant_fundamental_snapshot_report_period` (`report_period`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_index_member` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `index_name` VARCHAR(16) NOT NULL,
  `code` VARCHAR(16) NOT NULL,
  `in_date` DATE NOT NULL,
  `out_date` DATE DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_index_member` (`index_name`, `code`, `in_date`),
  KEY `ix_quant_index_member_code` (`code`),
  KEY `ix_quant_index_member_index_name` (`index_name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_pick` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `date` DATE NOT NULL,
  `code` VARCHAR(16) NOT NULL,
  `score` FLOAT NOT NULL,
  `rank` INT NOT NULL,
  `factors` JSON DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_pick_date_code` (`date`, `code`),
  KEY `ix_quant_pick_code` (`code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_pool` (
  `id` INT NOT NULL AUTO_INCREMENT,
  `kind` VARCHAR(16) NOT NULL,
  `ref` VARCHAR(32) DEFAULT NULL,
  `owner_id` VARCHAR(36) NOT NULL,
  `is_system` TINYINT(1) NOT NULL DEFAULT 0,
  `name` VARCHAR(64) NOT NULL,
  `min_list_days` INT NOT NULL,
  `created_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_pool_owner_name` (`owner_id`, `name`),
  KEY `ix_quant_pool_is_system` (`is_system`),
  KEY `ix_quant_pool_kind` (`kind`),
  KEY `ix_quant_pool_owner_id` (`owner_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_snapshot` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `code` VARCHAR(16) NOT NULL,
  `ts` DATETIME NOT NULL,
  `price` DECIMAL(12, 4) NOT NULL,
  `pct_chg` DECIMAL(9, 4) DEFAULT NULL,
  `volume` DECIMAL(20, 2) DEFAULT NULL,
  `amount` DECIMAL(20, 2) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_quant_snapshot_code` (`code`),
  KEY `ix_quant_snapshot_ts` (`ts`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_stock` (
  `code` VARCHAR(16) NOT NULL,
  `name` VARCHAR(64) NOT NULL,
  `industry` VARCHAR(64) NOT NULL,
  `is_watch` TINYINT(1) NOT NULL,
  `list_date` DATE DEFAULT NULL,
  `delist_date` DATE DEFAULT NULL,
  `is_st` TINYINT(1) NOT NULL DEFAULT 0,
  PRIMARY KEY (`code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_strategy` (
  `id` INT NOT NULL AUTO_INCREMENT,
  `owner_id` VARCHAR(36) NOT NULL,
  `is_system` TINYINT(1) NOT NULL DEFAULT 0,
  `name` VARCHAR(64) NOT NULL,
  `template` VARCHAR(32) NOT NULL,
  `kind` VARCHAR(16) NOT NULL,
  `params` JSON DEFAULT NULL,
  `enabled` TINYINT(1) NOT NULL DEFAULT 1,
  `created_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_strategy_owner_name` (`owner_id`, `name`),
  KEY `ix_quant_strategy_enabled` (`enabled`),
  KEY `ix_quant_strategy_is_system` (`is_system`),
  KEY `ix_quant_strategy_kind` (`kind`),
  KEY `ix_quant_strategy_owner_id` (`owner_id`),
  KEY `ix_quant_strategy_template` (`template`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_trade` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `user_id` VARCHAR(36) NOT NULL,
  `code` VARCHAR(16) NOT NULL,
  `trade_date` DATE NOT NULL,
  `side` VARCHAR(8) NOT NULL,
  `price` DECIMAL(12, 4) NOT NULL,
  `qty` DECIMAL(18, 4) NOT NULL,
  `fee` DECIMAL(18, 4) NOT NULL,
  `note` TEXT NOT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_quant_trade_code` (`code`),
  KEY `ix_quant_trade_user_id` (`user_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_trade_calendar` (
  `date` DATE NOT NULL,
  `is_open` TINYINT(1) NOT NULL,
  `source` VARCHAR(16) NOT NULL DEFAULT 'baostock',
  PRIMARY KEY (`date`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_valuation_snapshot` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `code` VARCHAR(16) NOT NULL,
  `data_date` DATE NOT NULL,
  `report_period` DATE DEFAULT NULL,
  `available_date` DATE NOT NULL,
  `source` VARCHAR(96) NOT NULL,
  `pe_ttm` FLOAT DEFAULT NULL,
  `pb` FLOAT DEFAULT NULL,
  `ps_ttm` FLOAT DEFAULT NULL,
  `dividend_yield` FLOAT DEFAULT NULL,
  `total_market_cap` DECIMAL(20, 2) DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_valuation_code_date_available`
    (`code`, `data_date`, `available_date`),
  KEY `ix_quant_valuation_snapshot_available_date` (`available_date`),
  KEY `ix_quant_valuation_snapshot_code` (`code`),
  KEY `ix_quant_valuation_snapshot_data_date` (`data_date`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_watchlist` (
  `user_id` VARCHAR(36) NOT NULL,
  `code` VARCHAR(16) NOT NULL,
  `created_at` DATETIME NOT NULL,
  PRIMARY KEY (`user_id`, `code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_backtest_run` (
  `id` INT NOT NULL AUTO_INCREMENT,
  `user_id` VARCHAR(36) NOT NULL,
  `strategy_id` INT NOT NULL,
  `params` JSON DEFAULT NULL,
  `costs` JSON DEFAULT NULL,
  `pool_id` INT DEFAULT NULL,
  `codes` JSON DEFAULT NULL,
  `start` DATE NOT NULL,
  `end` DATE NOT NULL,
  `metrics` JSON DEFAULT NULL,
  `created_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_quant_backtest_run_strategy_id` (`strategy_id`),
  KEY `ix_quant_backtest_run_user_id` (`user_id`),
  CONSTRAINT `fk_quant_backtest_run_strategy_id`
    FOREIGN KEY (`strategy_id`) REFERENCES `quant_strategy` (`id`)
    ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_pool_grant` (
  `pool_id` INT NOT NULL,
  `user_id` VARCHAR(36) NOT NULL,
  `can_edit` TINYINT(1) NOT NULL DEFAULT 0,
  `created_at` DATETIME NOT NULL,
  PRIMARY KEY (`pool_id`, `user_id`),
  KEY `ix_quant_pool_grant_user_id` (`user_id`),
  CONSTRAINT `quant_pool_grant_ibfk_1`
    FOREIGN KEY (`pool_id`) REFERENCES `quant_pool` (`id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_pool_member` (
  `pool_id` INT NOT NULL,
  `code` VARCHAR(16) NOT NULL,
  PRIMARY KEY (`pool_id`, `code`),
  CONSTRAINT `quant_pool_member_ibfk_1`
    FOREIGN KEY (`pool_id`) REFERENCES `quant_pool` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_research_plan` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `owner_id` VARCHAR(36) NOT NULL,
  `strategy_is_system` TINYINT(1) NOT NULL DEFAULT 0,
  `strategy_id` INT NOT NULL,
  `strategy_name` VARCHAR(64) NOT NULL,
  `template` VARCHAR(32) NOT NULL,
  `strategy_kind` VARCHAR(16) NOT NULL,
  `strategy_version` VARCHAR(64) NOT NULL,
  `params_snapshot` JSON NOT NULL,
  `plan_type` VARCHAR(32) NOT NULL,
  `code` VARCHAR(16) DEFAULT NULL,
  `pool_id` INT DEFAULT NULL,
  `data_date` DATE NOT NULL,
  `generated_at` DATETIME NOT NULL,
  `next_execution_date` DATE DEFAULT NULL,
  `valid_until` DATE DEFAULT NULL,
  `signal_type` VARCHAR(32) NOT NULL,
  `status` VARCHAR(32) NOT NULL,
  `status_reason` JSON NOT NULL,
  `price_adjustment` VARCHAR(16) NOT NULL DEFAULT 'forward',
  `signal_price` DECIMAL(12, 4) DEFAULT NULL,
  `entry_observation` JSON NOT NULL,
  `risk_rules` JSON NOT NULL,
  `take_profit` JSON NOT NULL,
  `native_exit` JSON NOT NULL,
  `exit_hits` JSON NOT NULL,
  `portfolio_summary` JSON DEFAULT NULL,
  `backtest_run_id` INT DEFAULT NULL,
  `backtest_evidence` JSON NOT NULL,
  `product_boundary` TEXT NOT NULL,
  `revision` INT NOT NULL DEFAULT 1,
  `supersedes_plan_id` BIGINT DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_quant_research_plan_owner_id` (`owner_id`),
  KEY `ix_quant_research_plan_strategy_is_system` (`strategy_is_system`),
  KEY `ix_quant_research_plan_strategy_id` (`strategy_id`),
  KEY `ix_quant_research_plan_template` (`template`),
  KEY `ix_quant_research_plan_plan_type` (`plan_type`),
  KEY `ix_quant_research_plan_code` (`code`),
  KEY `ix_quant_research_plan_data_date` (`data_date`),
  KEY `ix_quant_research_plan_generated_at` (`generated_at`),
  KEY `ix_quant_research_plan_status` (`status`),
  KEY `ix_quant_research_plan_backtest_run_id` (`backtest_run_id`),
  KEY `ix_quant_research_plan_supersedes_plan_id` (`supersedes_plan_id`),
  CONSTRAINT `fk_research_plan_backtest_run`
    FOREIGN KEY (`backtest_run_id`) REFERENCES `quant_backtest_run` (`id`)
    ON DELETE RESTRICT,
  CONSTRAINT `fk_research_plan_supersedes`
    FOREIGN KEY (`supersedes_plan_id`) REFERENCES `quant_research_plan` (`id`)
    ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_research_plan_item` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `plan_id` BIGINT NOT NULL,
  `code` VARCHAR(16) NOT NULL,
  `previous_weight` DECIMAL(12, 8) NOT NULL DEFAULT 0,
  `target_weight` DECIMAL(12, 8) NOT NULL DEFAULT 0,
  `change_type` VARCHAR(16) NOT NULL,
  `score` FLOAT DEFAULT NULL,
  `score_details` JSON DEFAULT NULL,
  `rank` INT DEFAULT NULL,
  `eligible` TINYINT(1) NOT NULL DEFAULT 1,
  `reasons` JSON NOT NULL,
  `risk_snapshot` JSON DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_research_plan_item` (`plan_id`, `code`),
  KEY `ix_quant_research_plan_item_plan_id` (`plan_id`),
  KEY `ix_quant_research_plan_item_code` (`code`),
  KEY `ix_quant_research_plan_item_change_type` (`change_type`),
  CONSTRAINT `fk_research_plan_item_plan`
    FOREIGN KEY (`plan_id`) REFERENCES `quant_research_plan` (`id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_signal` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `code` VARCHAR(16) NOT NULL,
  `date` DATE NOT NULL,
  `strategy_id` INT NOT NULL,
  `side` VARCHAR(8) NOT NULL,
  `price` DECIMAL(12, 4) DEFAULT NULL,
  `reason` JSON DEFAULT NULL,
  `plan_id` BIGINT DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_signal` (`code`, `date`, `strategy_id`, `side`),
  KEY `ix_quant_signal_code` (`code`),
  KEY `ix_quant_signal_date` (`date`),
  KEY `ix_quant_signal_strategy_id` (`strategy_id`),
  KEY `ix_quant_signal_plan_id` (`plan_id`),
  CONSTRAINT `fk_quant_signal_strategy_id`
    FOREIGN KEY (`strategy_id`) REFERENCES `quant_strategy` (`id`)
    ON DELETE CASCADE,
  CONSTRAINT `fk_quant_signal_plan_id`
    FOREIGN KEY (`plan_id`) REFERENCES `quant_research_plan` (`id`)
    ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_strategy_eval` (
  `id` INT NOT NULL AUTO_INCREMENT,
  `strategy_id` INT NOT NULL,
  `params` JSON DEFAULT NULL,
  `scope` VARCHAR(64) NOT NULL,
  `batch_id` VARCHAR(36) NOT NULL,
  `pool_id` INT DEFAULT NULL,
  `start` DATE NOT NULL,
  `end` DATE NOT NULL,
  `metrics` JSON DEFAULT NULL,
  `run_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_quant_strategy_eval_batch_id` (`batch_id`),
  KEY `ix_quant_strategy_eval_run_at` (`run_at`),
  KEY `ix_quant_strategy_eval_scope` (`scope`),
  KEY `ix_quant_strategy_eval_strategy_id` (`strategy_id`),
  CONSTRAINT `fk_quant_strategy_eval_strategy_id`
    FOREIGN KEY (`strategy_id`) REFERENCES `quant_strategy` (`id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_backtest_equity` (
  `id` BIGINT NOT NULL AUTO_INCREMENT,
  `run_id` INT NOT NULL,
  `date` DATE NOT NULL,
  `equity` DECIMAL(18, 8) NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_bt_equity_run_date` (`run_id`, `date`),
  KEY `ix_quant_backtest_equity_run_id` (`run_id`),
  CONSTRAINT `quant_backtest_equity_ibfk_1`
    FOREIGN KEY (`run_id`) REFERENCES `quant_backtest_run` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

START TRANSACTION;

-- 系统池和系统策略使用哨兵 UUID，不依赖共享 users 表中的真实用户。
INSERT INTO `quant_pool`
  (`id`, `kind`, `ref`, `owner_id`, `is_system`, `name`, `min_list_days`,
   `created_at`)
VALUES
  (1, 'index', 'hs300_zz500', '00000000-0000-0000-0000-000000000000', 1,
   '沪深300+中证500', 0, CURRENT_TIMESTAMP),
  (2, 'all', NULL, '00000000-0000-0000-0000-000000000000', 1,
   '全部A股', 60, CURRENT_TIMESTAMP),
  (3, 'index', 'hs300', '00000000-0000-0000-0000-000000000000', 1,
   '沪深300', 0, CURRENT_TIMESTAMP),
  (4, 'index', 'zz500', '00000000-0000-0000-0000-000000000000', 1,
   '中证500', 0, CURRENT_TIMESTAMP);

INSERT INTO `quant_strategy`
  (`id`, `owner_id`, `is_system`, `name`, `template`, `kind`, `params`,
   `enabled`, `created_at`)
VALUES
  (1, '00000000-0000-0000-0000-000000000000', 1, '双均线趋势策略',
   'ma_cross', 'single', '{}', 1, CURRENT_TIMESTAMP),
  (2, '00000000-0000-0000-0000-000000000000', 1, '价格突破策略',
   'breakout', 'single', '{}', 1, CURRENT_TIMESTAMP),
  (3, '00000000-0000-0000-0000-000000000000', 1,
   '上升趋势中的超跌反弹策略', 'mean_reversion', 'single', '{}', 1,
   CURRENT_TIMESTAMP),
  (4, '00000000-0000-0000-0000-000000000000', 1,
   '缩量整理后的放量突破策略', 'volume_breakout', 'single', '{}', 1,
   CURRENT_TIMESTAMP),
  (5, '00000000-0000-0000-0000-000000000000', 1, '强势股票轮动策略',
   'momentum_rotation', 'portfolio', '{}', 1, CURRENT_TIMESTAMP),
  (6, '00000000-0000-0000-0000-000000000000', 1,
   '多指标综合评分持有策略', 'multifactor_hold', 'portfolio', '{}', 1,
   CURRENT_TIMESTAMP);

-- 仅在所有建表和种子数据写入成功后标记 schema 版本。
INSERT INTO `alembic_version` (`version_num`)
VALUES ('0013_research_plan');

COMMIT;
