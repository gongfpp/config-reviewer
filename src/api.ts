import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  Defaults,
  ExportRow,
  GatewayConfig,
  ReviewRequest,
  ReviewSummary,
  RuleSet,
  ScanResult,
  SystemFileSummary,
  TaskDetail,
  TaskSummary,
  Template,
} from "./types";

export const loadConfig = () => invoke<AppConfig>("load_config");

export const loadDefaults = () => invoke<Defaults>("load_defaults");

export const saveSettings = (config: AppConfig, apiKey?: string) =>
  invoke<void>("save_settings", { config, apiKey: apiKey ?? null });

export const hasApiKey = () => invoke<boolean>("has_api_key");

export const clearApiKey = () => invoke<void>("clear_api_key");

export const scanDirectory = (args: {
  requestId: string;
  path: string;
  ruleSet: RuleSet;
  recursive: boolean;
  matchedOnly: boolean;
  threshold: number;
  chunkSize: number;
}) => invoke<ScanResult>("scan_directory", args);

export const scanUploads = (args: {
  requestId: string;
  paths: string[];
  ruleSet: RuleSet;
  matchedOnly: boolean;
  threshold: number;
  chunkSize: number;
}) => invoke<ScanResult>("scan_uploads", args);

export const scanText = (args: {
  requestId: string;
  content: string;
  system: string;
  templateId: string;
  ruleSet: RuleSet;
  threshold: number;
  chunkSize: number;
}) => invoke<ScanResult>("scan_text", args);

export const testConnection = (cfg: GatewayConfig, apiKey?: string) =>
  invoke<string>("test_connection", { cfg, apiKey: apiKey ?? null });

export const cancelReview = (taskId: string) => invoke<void>("cancel_review", { taskId });

export const runReview = (request: ReviewRequest) => invoke<ReviewSummary>("run_review", { request });

export const summarizeSystem = (args: {
  cfg: GatewayConfig;
  apiKey?: string;
  systemName: string;
  files: SystemFileSummary[];
}) =>
  invoke<string>("summarize_system", {
    cfg: args.cfg,
    apiKey: args.apiKey ?? null,
    systemName: args.systemName,
    files: args.files,
  });

export const exportXlsx = (path: string, rows: ExportRow[]) =>
  invoke<number>("export_xlsx", { path, rows });

export const saveTask = (args: {
  sourceType: string;
  sourceRef: string;
  ruleSet: RuleSet;
  templates: Template[];
  summary: ReviewSummary;
}) => invoke<void>("save_task", args);

export const listHistory = () => invoke<TaskSummary[]>("list_history");

export const getHistory = (id: string) => invoke<TaskDetail | null>("get_history", { id });

export const deleteHistory = (id: string) => invoke<void>("delete_history", { id });

export const exportText = (path: string, content: string) => invoke<void>("export_text", { path, content });

export const cancelScan = (taskId: string) => invoke<void>("cancel_scan", { taskId });
