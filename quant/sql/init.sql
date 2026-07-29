-- quant 全新数据库初始化脚本（MySQL 5.7+/8.0）
--
-- 仅用于空数据库。已有数据库必须执行 `uv run alembic upgrade head`，不要用本
-- 脚本覆盖或补建，否则 Alembic 无法可靠判断已经执行过哪些数据迁移。
-- 本脚本只管理 quant_* 表；与主服务共享、由 app/auth.py 只读访问的 users 表
-- 不属于 quant schema，不在这里创建。
--
-- Schema revision: 0019_user_settings

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
  `spec_schema_version` INT NOT NULL DEFAULT 1,
  `spec` JSON NOT NULL,
  `spec_hash` VARCHAR(64) NOT NULL,
  `research_status` VARCHAR(32) NOT NULL DEFAULT 'unverified',
  `enabled` TINYINT(1) NOT NULL DEFAULT 1,
  `created_at` DATETIME NOT NULL,
  `updated_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_strategy_owner_name` (`owner_id`, `name`),
  KEY `ix_quant_strategy_enabled` (`enabled`),
  KEY `ix_quant_strategy_is_system` (`is_system`),
  KEY `ix_quant_strategy_kind` (`kind`),
  KEY `ix_quant_strategy_owner_id` (`owner_id`),
  KEY `ix_quant_strategy_research_status` (`research_status`),
  KEY `ix_quant_strategy_spec_hash` (`spec_hash`),
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

-- 旁路缓存:仅存 data-quality 聚合 JSON,与行情/财务源表无 FK
CREATE TABLE `quant_data_quality_cache` (
  `scope` VARCHAR(32) NOT NULL,
  `as_of` DATE NOT NULL,
  `payload` JSON NOT NULL,
  `computed_at` DATETIME NOT NULL,
  PRIMARY KEY (`scope`)
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

CREATE TABLE `quant_user_settings` (
  `user_id` VARCHAR(36) NOT NULL,
  `can_trade_bse` TINYINT(1) NOT NULL DEFAULT 0,
  `updated_at` DATETIME NOT NULL,
  PRIMARY KEY (`user_id`)
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
  `strategy_spec_snapshot` JSON DEFAULT NULL,
  `strategy_spec_hash` VARCHAR(64) DEFAULT NULL,
  `compiler_version` VARCHAR(64) DEFAULT NULL,
  `component_versions` JSON DEFAULT NULL,
  `data_fingerprint` VARCHAR(64) DEFAULT NULL,
  `universe_fingerprint` VARCHAR(64) DEFAULT NULL,
  `cost_fingerprint` VARCHAR(64) DEFAULT NULL,
  `execution_fingerprint` VARCHAR(64) DEFAULT NULL,
  `status` VARCHAR(16) NOT NULL DEFAULT 'done',
  `error` TEXT DEFAULT NULL,
  `request_snapshot` JSON DEFAULT NULL,
  `started_at` DATETIME DEFAULT NULL,
  `finished_at` DATETIME DEFAULT NULL,
  `created_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_quant_backtest_run_execution_fingerprint` (`execution_fingerprint`),
  KEY `ix_quant_backtest_run_strategy_spec_hash` (`strategy_spec_hash`),
  KEY `ix_quant_backtest_run_strategy_id` (`strategy_id`),
  KEY `ix_quant_backtest_run_user_id` (`user_id`),
  KEY `ix_quant_backtest_run_status` (`status`),
  CONSTRAINT `fk_quant_backtest_run_strategy_id`
    FOREIGN KEY (`strategy_id`) REFERENCES `quant_strategy` (`id`)
    ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_experiment` (
  `id` INT NOT NULL AUTO_INCREMENT,
  `owner_id` VARCHAR(36) NOT NULL,
  `permanent_candidate_id` VARCHAR(64) NOT NULL,
  `family_id` VARCHAR(64) DEFAULT NULL,
  `title` VARCHAR(128) NOT NULL,
  `hypothesis` TEXT NOT NULL,
  `strategy_id` INT DEFAULT NULL,
  `frozen_spec_snapshot` JSON NOT NULL,
  `frozen_spec_hash` VARCHAR(64) NOT NULL,
  `identity_hash` VARCHAR(64) NOT NULL,
  `validation_snapshot` JSON DEFAULT NULL,
  `universe_snapshot` JSON DEFAULT NULL,
  `cost_snapshot` JSON DEFAULT NULL,
  `status` VARCHAR(16) NOT NULL DEFAULT 'design',
  `created_at` DATETIME NOT NULL,
  `updated_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_experiment_owner_candidate` (`owner_id`, `permanent_candidate_id`),
  KEY `ix_quant_experiment_owner_id` (`owner_id`),
  KEY `ix_quant_experiment_identity_hash` (`identity_hash`),
  KEY `ix_quant_experiment_status` (`status`),
  KEY `ix_quant_experiment_strategy_id` (`strategy_id`),
  CONSTRAINT `fk_experiment_strategy_id`
    FOREIGN KEY (`strategy_id`) REFERENCES `quant_strategy` (`id`)
    ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `quant_experiment_trial` (
  `id` INT NOT NULL AUTO_INCREMENT,
  `experiment_id` INT NOT NULL,
  `trial_index` INT NOT NULL,
  `param_patch` JSON DEFAULT NULL,
  `backtest_run_id` INT DEFAULT NULL,
  `outcome` VARCHAR(16) NOT NULL DEFAULT 'error',
  `metrics_summary` JSON DEFAULT NULL,
  `error` TEXT DEFAULT NULL,
  `data_fingerprint` VARCHAR(64) DEFAULT NULL,
  `universe_fingerprint` VARCHAR(64) DEFAULT NULL,
  `cost_fingerprint` VARCHAR(64) DEFAULT NULL,
  `execution_fingerprint` VARCHAR(64) DEFAULT NULL,
  `oos_revealed_at` DATETIME DEFAULT NULL,
  `created_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_experiment_trial_index` (`experiment_id`, `trial_index`),
  KEY `ix_quant_experiment_trial_experiment_id` (`experiment_id`),
  KEY `ix_quant_experiment_trial_backtest_run_id` (`backtest_run_id`),
  KEY `ix_quant_experiment_trial_outcome` (`outcome`),
  CONSTRAINT `fk_experiment_trial_experiment`
    FOREIGN KEY (`experiment_id`) REFERENCES `quant_experiment` (`id`)
    ON DELETE RESTRICT,
  CONSTRAINT `fk_experiment_trial_backtest`
    FOREIGN KEY (`backtest_run_id`) REFERENCES `quant_backtest_run` (`id`)
    ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

-- 试验达标后的证据推进待办(不自动改 evidence_status;用户采纳才推进)
CREATE TABLE `quant_evidence_promotion` (
  `id` INT NOT NULL AUTO_INCREMENT,
  `owner_id` VARCHAR(36) NOT NULL,
  `strategy_id` INT NOT NULL,
  `experiment_id` INT NOT NULL,
  `trial_id` INT NOT NULL,
  `backtest_run_id` INT NOT NULL,
  `status` VARCHAR(16) NOT NULL DEFAULT 'pending',
  `suggested_target` VARCHAR(16) NOT NULL,
  `quality_checks` JSON DEFAULT NULL,
  `metrics_summary` JSON DEFAULT NULL,
  `created_at` DATETIME NOT NULL,
  `resolved_at` DATETIME DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_evidence_promotion_trial` (`trial_id`),
  KEY `ix_quant_evidence_promotion_owner_id` (`owner_id`),
  KEY `ix_quant_evidence_promotion_strategy_id` (`strategy_id`),
  KEY `ix_quant_evidence_promotion_experiment_id` (`experiment_id`),
  KEY `ix_quant_evidence_promotion_status` (`status`),
  CONSTRAINT `fk_evidence_promotion_strategy`
    FOREIGN KEY (`strategy_id`) REFERENCES `quant_strategy` (`id`)
    ON DELETE CASCADE,
  CONSTRAINT `fk_evidence_promotion_experiment`
    FOREIGN KEY (`experiment_id`) REFERENCES `quant_experiment` (`id`)
    ON DELETE CASCADE,
  CONSTRAINT `fk_evidence_promotion_trial`
    FOREIGN KEY (`trial_id`) REFERENCES `quant_experiment_trial` (`id`)
    ON DELETE CASCADE,
  CONSTRAINT `fk_evidence_promotion_run`
    FOREIGN KEY (`backtest_run_id`) REFERENCES `quant_backtest_run` (`id`)
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
  `strategy_spec_snapshot` JSON DEFAULT NULL,
  `strategy_spec_hash` VARCHAR(64) DEFAULT NULL,
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
  KEY `ix_quant_research_plan_strategy_spec_hash` (`strategy_spec_hash`),
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
  `spec_hash` VARCHAR(64) DEFAULT NULL,
  `plan_id` BIGINT DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_signal` (`code`, `date`, `strategy_id`, `side`),
  KEY `ix_quant_signal_code` (`code`),
  KEY `ix_quant_signal_date` (`date`),
  KEY `ix_quant_signal_spec_hash` (`spec_hash`),
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
  `spec_hash` VARCHAR(64) DEFAULT NULL,
  `run_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_quant_strategy_eval_batch_id` (`batch_id`),
  KEY `ix_quant_strategy_eval_run_at` (`run_at`),
  KEY `ix_quant_strategy_eval_scope` (`scope`),
  KEY `ix_quant_strategy_eval_spec_hash` (`spec_hash`),
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
   `spec_schema_version`, `spec`, `spec_hash`, `research_status`,
   `enabled`, `created_at`, `updated_at`)
VALUES
  (1, '00000000-0000-0000-0000-000000000000', 1, '双均线趋势策略',
   'ma_cross', 'single', '{}', 1, '{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"left":{"input":{"name":"close","op":"field"},"op":"ma","window":5},"op":"gt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":20}},"reason_code":"fast_ma_above_slow"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-TRD-01","evidence_status":"unverified","hypothesis":"短期均价高于长期均价时，趋势延续概率可能高于简单持有基线。","sources":[{"book":"股市趋势技术分析","candidate_id":"TREND-08"}]},"native_exit":{"condition":{"left":{"input":{"name":"close","op":"field"},"op":"ma","window":5},"op":"lte","right":{"input":{"name":"close","op":"field"},"op":"ma","window":20}},"reason_code":"fast_ma_not_above_slow"},"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}}',
   '9d151ac891ff6b8f742b10c2e8ea08c6d63f03bf6a87549c0eb416ed00747222', 'unverified', 1, CURRENT_TIMESTAMP,
   CURRENT_TIMESTAMP),
  (2, '00000000-0000-0000-0000-000000000000', 1, '价格突破策略',
   'breakout', 'single', '{}', 1, '{"data_requirements":[{"availability":"daily_close","field":"close","required":true},{"availability":"daily_close","field":"high","required":true},{"availability":"daily_close","field":"low","required":true}],"entry":{"condition":{"left":{"name":"close","op":"field"},"op":"gt","right":{"input":{"name":"high","op":"field"},"op":"rolling_max","shift":1,"window":20}},"reason_code":"close_above_prior_high"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-TRD-02","evidence_status":"unverified","hypothesis":"收盘突破历史区间上沿后可能延续，跌破较短退出通道表示假设失效。","sources":[{"book":"股市趋势技术分析","candidate_id":"TREND-03"}]},"native_exit":{"condition":{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"low","op":"field"},"op":"rolling_min","shift":1,"window":10}},"reason_code":"close_below_prior_low"},"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}}',
   'b5ca04f041a7e1acc474ab1f7a725b81ca3fc4524ea49a50b8f177378ec42790', 'unverified', 1, CURRENT_TIMESTAMP,
   CURRENT_TIMESTAMP),
  (3, '00000000-0000-0000-0000-000000000000', 1, '上升趋势中的超跌反弹策略',
   'mean_reversion', 'single', '{}', 1, '{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"args":[{"left":{"input":{"name":"close","op":"field"},"op":"rsi","window":14},"op":"lt","right":{"op":"literal","value":30.0}},{"left":{"name":"close","op":"field"},"op":"gt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":60}}],"op":"all"},"reason_code":"uptrend_oversold"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-REV-06","evidence_status":"unverified","hypothesis":"长期趋势向上时的短期超卖可能均值修复，修复完成或趋势失效时退出。","sources":[{"book":"量化交易从入门到精通","candidate_id":"QTP-003"}]},"native_exit":{"condition":{"args":[{"left":{"input":{"name":"close","op":"field"},"op":"rsi","window":14},"op":"gt","right":{"op":"literal","value":55.0}},{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":60}}],"op":"any"},"reason_code":"reversion_complete_or_trend_failed"},"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}}',
   '91a66822bc238722d6e1ab699444f5e0e0ecb3fcab2bfce67e92b99e2f66d90f', 'unverified', 1, CURRENT_TIMESTAMP,
   CURRENT_TIMESTAMP),
  (4, '00000000-0000-0000-0000-000000000000', 1, '缩量整理后的放量突破策略',
   'volume_breakout', 'single', '{}', 1, '{"data_requirements":[{"availability":"daily_close","field":"close","required":true},{"availability":"daily_close","field":"high","required":true},{"availability":"daily_close","field":"low","required":true},{"availability":"daily_close","field":"volume","required":true}],"entry":{"condition":{"args":[{"left":{"left":{"left":{"input":{"name":"high","op":"field"},"op":"rolling_max","shift":1,"window":20},"op":"subtract","right":{"input":{"name":"low","op":"field"},"op":"rolling_min","shift":1,"window":20}},"op":"divide","right":{"name":"close","op":"field"}},"op":"lte","right":{"op":"literal","value":0.15}},{"left":{"input":{"name":"volume","op":"field"},"op":"rolling_mean","shift":1,"window":5},"op":"lt","right":{"input":{"name":"volume","op":"field"},"op":"rolling_mean","shift":1,"window":20}},{"left":{"name":"volume","op":"field"},"op":"gt","right":{"left":{"op":"literal","value":2.0},"op":"multiply","right":{"input":{"name":"volume","op":"field"},"op":"rolling_mean","shift":1,"window":20}}},{"left":{"name":"close","op":"field"},"op":"gt","right":{"input":{"name":"high","op":"field"},"op":"rolling_max","shift":1,"window":20}}],"op":"all"},"reason_code":"contracted_volume_breakout"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-TRD-04","evidence_status":"unverified","hypothesis":"价格和成交收缩后的放量向上突破可能形成趋势，平台下沿或 ATR 风险线失效。","sources":[{"book":"量化交易从入门到精通","candidate_id":"QTP-002"}]},"native_exit":{"condition":{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"low","op":"field"},"op":"rolling_min","shift":1,"window":20}},"reason_code":"close_below_platform_low"},"overlays":{"risk":{"atr_period":14,"enabled":true,"trailing":true,"type":"atr_multiple","value":2.0},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}}',
   'b0c2f36d28eba22533ab7f0b0caadb4644159ee0d74ebd4870ec13c73cc8d1f1', 'unverified', 1, CURRENT_TIMESTAMP,
   CURRENT_TIMESTAMP),
  (5, '00000000-0000-0000-0000-000000000000', 1, '强势股票轮动策略',
   'momentum_rotation', 'portfolio', '{}', 1, '{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"op":"literal","value":true},"reason_code":"eligible_for_ranking"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"portfolio","metadata":{"canonical_id":"CAN-TRD-05","evidence_status":"unverified","hypothesis":"横截面中短期动量较强的股票可能延续，每周轮动并用短均线控制趋势失效。","sources":[{"book":"股票大作手回忆录","candidate_id":"LIV-04"}]},"native_exit":null,"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":10,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"rebalance":{"frequency":"weekly","interval_days":null},"risk_filter":{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":20}},"score":{"left":{"left":{"op":"literal","value":0.6},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":20}},"op":"add","right":{"left":{"op":"literal","value":0.4},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":60}}},"selection":{"n":10,"type":"top_n"},"type":"portfolio","weighting":{"type":"equal"}},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}}',
   '53614b11d09dce00108cfb7a69bdc49f079b5ef80830fd7a80dbf45b62e712ba', 'unverified', 1, CURRENT_TIMESTAMP,
   CURRENT_TIMESTAMP),
  (6, '00000000-0000-0000-0000-000000000000', 1, '多指标综合评分持有策略',
   'multifactor_hold', 'portfolio', '{}', 1, '{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"op":"literal","value":true},"reason_code":"eligible_for_ranking"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"portfolio","metadata":{"canonical_id":"CAN-PORT-04","evidence_status":"unverified","hypothesis":"中短期动量与均线斜率的组合排序可能比单因子等权基线更稳定。","sources":[{"book":"打开量化投资的黑箱","candidate_id":"BLACKBOX-ALPHA-01"}]},"native_exit":null,"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":20,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"rebalance":{"frequency":"monthly","interval_days":null},"risk_filter":null,"score":{"left":{"left":{"left":{"op":"literal","value":0.5},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":20}},"op":"add","right":{"left":{"op":"literal","value":0.3},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":60}}},"op":"add","right":{"left":{"op":"literal","value":0.2},"op":"multiply","right":{"input":{"input":{"name":"close","op":"field"},"op":"ma","window":20},"op":"return","window":5}}},"selection":{"n":20,"type":"top_n"},"type":"portfolio","weighting":{"type":"equal"}},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}}',
   '8d00c45330301d818cd97c1f9163da61f8fff519036de1c4e0f6e0a7657b4ffe', 'unverified', 1, CURRENT_TIMESTAMP,
   CURRENT_TIMESTAMP);

-- 仅在所有建表和种子数据写入成功后标记 schema 版本。
INSERT INTO `alembic_version` (`version_num`)
VALUES ('0019_user_settings');

COMMIT;
