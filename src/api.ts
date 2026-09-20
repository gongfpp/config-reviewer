import { invoke } from "@tauri-apps/api/core";
import type {
  Defaults,
  GatewayConfig,
  ReviewRequest,
  ReviewSummary,
  RuleSet,
  ScanResult,
  TaskDetail,
  TaskSummary,
  Template,
} from "./types";

export const getDefaults = () => invoke<Defaults>("get_defaults");

export const scanDirectory = (args: {
  path: string;
  ruleSet: RuleSet;
  recursive: boolean;
  matchedOnly: boolean;
  threshold: number;
  chunkSize: number;
}) => invoke<ScanResult>("scan_directory", args);

export const scanUploads = (args: {
  paths: string[];
  ruleSet: RuleSet;
  matchedOnly: boolean;
  threshold: number;
  chunkSize: number;
}) => invoke<ScanResult>("scan_uploads", args);

export const testConnection = (cfg: GatewayConfig) =>
  invoke<string>("test_connection", { cfg });

export const runReview = (request: ReviewRequest) =>
  invoke<ReviewSummary>("run_review", { request });

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
