export interface SystemType {
  id: string;
  name: string;
  color: string;
  default_template_id: string;
}

export interface Template {
  id: string;
  name: string;
  content: string;
  builtin: boolean;
  overridden: boolean;
}

export interface Rule {
  id: string;
  pattern: string;
  system: string;
  template_id: string;
  enabled: boolean;
}

export interface RuleSet {
  id: string;
  name: string;
  builtin: boolean;
  rules: Rule[];
}

export interface ScanEntry {
  path: string;
  abs_path: string;
  name: string;
  ext: string;
  kind: string;
  size: number;
  system: string | null;
  rule_id: string | null;
  template_id: string | null;
  matched: boolean;
  selected: boolean;
  chunks: number;
  parse_status: string;
  parse_error: string | null;
  preview: string;
  text_len: number;
}

export interface ScanResult {
  task_id: string;
  source_type: string;
  source_ref: string;
  total_files: number;
  matched_files: number;
  total_bytes: number;
  entries: ScanEntry[];
  workspace: string;
  rule_set: RuleSet;
  templates: Template[];
  systems: SystemType[];
}

export interface GatewayConfig {
  base_url: string;
  model: string;
  api_key: string;
  allow_http: boolean;
  timeout_secs: number;
}

export interface Citation {
  file: string;
  locator: string;
  snippet: string;
}

export interface FileResult {
  file: string;
  system: string | null;
  template_id: string;
  verdict: string;
  basis: string[];
  risks: string[];
  remediations: string[];
  citations: Citation[];
  chunks: number;
  duration_ms: number;
  raw: string;
  error: string | null;
}

export interface ReviewFileInput {
  path: string;
  abs_path: string;
  name: string;
  kind: string;
  system: string | null;
  template_id: string;
  template_content: string;
}

export type Granularity = "per_file" | "per_file_system" | "single";
export type OutputFormat = "standard" | "checklist" | "brief" | "json";

export interface ReviewRequest {
  task_id: string;
  workspace: string;
  files: ReviewFileInput[];
  cfg: GatewayConfig;
  concurrency: number;
  chunk_threshold: number;
  chunk_size: number;
  granularity: Granularity;
  output_format: OutputFormat;
  retry_count: number;
}

export interface ReviewProgress {
  task_id: string;
  index: number;
  total: number;
  file: string;
  phase: string;
  delta: string | null;
  result: FileResult | null;
}

export interface ReviewSummary {
  task_id: string;
  results: FileResult[];
  pass: number;
  conditional: number;
  block: number;
  fail: number;
  cancelled: boolean;
}

export interface TaskSummary {
  id: string;
  created_at: number;
  source_type: string;
  source_ref: string;
  systems: string[];
  total_files: number;
  pass: number;
  conditional: number;
  block: number;
  fail: number;
}

export interface TaskDetail {
  summary: TaskSummary;
  files: FileResult[];
  rule_set: RuleSet | null;
  templates: Template[];
}

export interface Defaults {
  systems: SystemType[];
  templates: Template[];
  rule_sets: RuleSet[];
}

/** 应用配置（Rust `AppConfig`）；不含 API Key。 */
export interface AppConfig {
  base_url: string;
  model: string;
  allow_http: boolean;
  timeout_secs: number;
  concurrency: number;
  max_file_mb: number;
  max_files: number;
  chunk_threshold: number;
  chunk_size: number;
  retry_count: number;
  granularity: Granularity;
  output_format: OutputFormat;
  history_retention_days: number;
  systems: SystemType[];
  templates: Template[];
  rule_sets: RuleSet[];
}

export interface ExportRow {
  system: string;
  file: string;
  file_type: string;
  rule: string;
  verdict: string;
  citations: string;
  time: string;
}

export interface SystemFileSummary {
  file: string;
  verdict: string;
  risks: string[];
}
