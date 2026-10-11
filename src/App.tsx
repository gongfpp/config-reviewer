import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import "./App.css";
import ReviewModules from "./ReviewModules";
import * as api from "./api";
import type {
  AppConfig,
  Defaults,
  ExportRow,
  FileResult,
  GatewayConfig,
  Granularity,
  OutputFormat,
  ReviewFileInput,
  ReviewProgress,
  ReviewSummary,
  ScanEntry,
  ScanResult,
  ScanProgress,
  SystemFileSummary,
  TaskDetail,
  TaskSummary,
} from "./types";

const GRANULARITY_LABEL: Record<Granularity, string> = {
  per_file: "逐文件",
  per_file_system: "逐文件 + 按模块汇总",
  single: "同模块合并评审",
};

const OUTPUT_FORMAT_LABEL: Record<OutputFormat, string> = {
  standard: "标准结论",
  checklist: "检查表",
  brief: "简洁摘要",
  json: "JSON",
};

function recomputeSummary(taskId: string, results: FileResult[], cancelled: boolean): ReviewSummary {
  let pass = 0;
  let conditional = 0;
  let block = 0;
  let fail = 0;
  for (const r of results) {
    if (r.error) fail += 1;
    else if (r.verdict === "通过") pass += 1;
    else if (r.verdict === "有条件通过") conditional += 1;
    else if (r.verdict === "阻断") block += 1;
    else fail += 1;
  }
  return { task_id: taskId, results, pass, conditional, block, fail, cancelled };
}

type View = "task" | "history" | "templates" | "settings";

function kindLabel(kind: string) {
  switch (kind) {
    case "json":
      return "JSON";
    case "excel":
      return "Excel";
    case "word":
      return "Word";
    case "text":
      return "文本";
    default:
      return kind;
  }
}

function formatTime(ms: number) {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function verdictClass(verdict: string) {
  if (verdict === "通过") return "pass";
  if (verdict === "有条件通过") return "conditional";
  if (verdict === "阻断") return "block";
  return "fail";
}

export default function App() {
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [view, setView] = useState<View>("task");
  const [activeRuleSetId, setActiveRuleSetId] = useState("");

  const [appVersion, setAppVersion] = useState("");
  useEffect(() => { void getVersion().then(setAppVersion); }, []);
  const [source, setSource] = useState<"upload" | "dir" | "text">("upload");
  const [uploadPaths, setUploadPaths] = useState<string[]>([]);
  const [dirPath, setDirPath] = useState("");
  const [recursive, setRecursive] = useState(true);
  const [matchedOnly, setMatchedOnly] = useState(true);
  const [textContent, setTextContent] = useState("");
  const [textSystem, setTextSystem] = useState("xxl");

  const [step, setStep] = useState(1);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [scanning, setScanning] = useState(false);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [scanError, setScanError] = useState("");
  const [scanCancelling, setScanCancelling] = useState(false);
  const [scanPage, setScanPage] = useState(0);
  const activeScanRef = useRef<string | null>(null);
  const [reviewTotal, setReviewTotal] = useState(0);
  const [reviewStage, setReviewStage] = useState("");
  const [connectionTesting, setConnectionTesting] = useState(false);
  const [connectionResult, setConnectionResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [exporting, setExporting] = useState(false);
  const [summarizing, setSummarizing] = useState<string | null>(null);
  const [preview, setPreview] = useState("选择文件可预览解析后的文本…");

  const [apiKey, setApiKey] = useState("");
  const [hasKey, setHasKey] = useState(false);

  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Record<number, { file: string; phase: string; text: string }>>({});
  const [results, setResults] = useState<FileResult[]>([]);
  const [summary, setSummary] = useState<ReviewSummary | null>(null);
  const [detail, setDetail] = useState<FileResult | null>(null);
  const [summaries, setSummaries] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState("");

  const [history, setHistory] = useState<TaskSummary[]>([]);
  const [historyDetail, setHistoryDetail] = useState<TaskDetail | null>(null);
  const [historySearch, setHistorySearch] = useState("");
  const [defaults, setDefaults] = useState<Defaults | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const noticeTimer = useRef<number | null>(null);
  const activeReviewRef = useRef<string | null>(null);
  const analysisRevisionRef = useRef(0);

  const toast = useCallback((message: string) => {
    setNotice(message);
    if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(""), 2400);
  }, []);

  useEffect(() => {
    Promise.all([api.loadConfig(), api.hasApiKey(), api.loadDefaults()])
      .then(([cfg, key, defs]) => {
        setAppConfig(cfg);
        setActiveRuleSetId(cfg.rule_sets[0]?.id ?? "");
        setTextSystem(cfg.systems[1]?.id ?? cfg.systems[0]?.id ?? "other");
        setHasKey(key);
        setDefaults(defs);
      })
      .catch((e) => toast(String(e)));
  }, [toast]);

  const refreshHistory = useCallback(() => {
    api.listHistory().then(setHistory).catch(() => undefined);
  }, []);

  useEffect(() => {
    refreshHistory();
  }, [refreshHistory]);

  useEffect(() => {
    const unlisten = listen<ReviewProgress>("review://progress", (event) => {
      const p = event.payload;
      if (p.task_id !== activeReviewRef.current) return;
      if (p.result) {
        setResults((prev) => {
          const next = prev.filter((r) => r.file !== p.result!.file);
          next.push(p.result!);
          return next;
        });
        setProgress((prev) => ({
          ...prev,
          [p.index]: { file: p.file, phase: "done", text: prev[p.index]?.text ?? "" },
        }));
        return;
      }
      if (p.delta) {
        setProgress((prev) => {
          const existing = prev[p.index]?.text ?? "";
          return {
            ...prev,
            [p.index]: { file: p.file, phase: p.phase, text: (existing + p.delta).slice(-4000) },
          };
        });
      } else {
        setProgress((prev) => ({
          ...prev,
          [p.index]: { file: p.file, phase: p.phase, text: prev[p.index]?.text ?? "" },
        }));
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const systems = appConfig?.systems ?? [];
  const templates = appConfig?.templates ?? [];
  useEffect(() => {
    if (!systems.some((s) => s.id === textSystem)) setTextSystem(systems[0]?.id ?? "");
  }, [systems, textSystem]);
  const changeRuleSet = (id: string) => {
    setActiveRuleSetId(id);
    setScan(null);
    setStep(1);
    setConfirmOpen(false);
  };
  const ruleSet = useMemo(
    () => appConfig?.rule_sets.find((r) => r.id === activeRuleSetId) ?? appConfig?.rule_sets[0] ?? null,
    [appConfig, activeRuleSetId]
  );
  const systemName = (id: string | null) => systems.find((s) => s.id === id)?.name ?? id ?? "—";
  const systemClass = (id: string | null) => id ?? "other";
  const sysStyle = (id: string | null): CSSProperties | undefined => {
    const color = systems.find((s) => s.id === id)?.color;
    return color ? ({ "--sys-color": color } as CSSProperties) : undefined;
  };

  // 结果分组：内置/自定义系统 + 结果中出现的未知系统 + 兜底 other，避免自定义系统结果被隐藏。
  const groupSystems = useMemo(() => {
    const ids: string[] = [];
    const push = (id: string) => {
      if (id && !ids.includes(id)) ids.push(id);
    };
    systems.forEach((s) => push(s.id));
    results.forEach((r) => push(r.system ?? "other"));
    push("other");
    return ids;
  }, [systems, results]);

  const filteredHistory = useMemo(() => {
    const q = historySearch.trim().toLowerCase();
    if (!q) return history;
    return history.filter(
      (h) =>
        h.source_ref.toLowerCase().includes(q) ||
        h.source_type.toLowerCase().includes(q) ||
        h.systems.some((s) => (systems.find((x) => x.id === s)?.name ?? s).toLowerCase().includes(q))
    );
  }, [history, historySearch, systems]);

  const gatewayCfg = (): GatewayConfig => ({
    base_url: appConfig?.base_url ?? "",
    model: appConfig?.model ?? "",
    api_key: "",
    allow_http: appConfig?.allow_http ?? true,
    timeout_secs: appConfig?.timeout_secs ?? 180,
    api_key_header: appConfig?.api_key_header ?? "",
    proxy_url: appConfig?.proxy_url ?? "",
    stream_enabled: appConfig?.stream_enabled ?? true,
  });

  const [configDirty, setConfigDirty] = useState(false);
  const updateConfig = (patch: Partial<AppConfig>) => {
    setAppConfig((prev) => (prev ? { ...prev, ...patch } : prev));
    setConfigDirty(true);
    if (["base_url", "model", "api_key_header", "proxy_url", "allow_http"].some((key) => key in patch)) setConnectionResult(null);
    if (patch.systems || patch.templates || patch.rule_sets) {
      setScan(null);
      setStep(1);
    }
  };
  const moduleTemplate = (id: string | null) => templates.find((t) => t.id === systems.find((s) => s.id === id)?.default_template_id);
  const mixedModuleMerge = appConfig?.granularity === "single" && new Set((scan?.entries ?? []).filter((e) => e.selected).map((e) => e.system)).size > 1;
  const selectedFormats = Array.from(new Set((scan?.entries ?? []).filter((e) => e.selected).map((e) => OUTPUT_FORMAT_LABEL[moduleTemplate(e.system)?.output_format ?? "standard"])));


  const pickFiles = async () => {
    const selected = await open({ multiple: true, directory: false });
    if (!selected) return;
    setUploadPaths(Array.isArray(selected) ? selected : [selected]);
  };

  const pickDir = async () => {
    const selected = await open({ directory: true });
    if (typeof selected === "string") setDirPath(selected);
  };

  const doScan = async () => {
    if (running || scanning) return toast("请先等待当前扫描或评审完成。");
    if (!ruleSet || !appConfig) return;
    const requestId = crypto.randomUUID();
    activeScanRef.current = requestId;
    setScanning(true);
    setScanCancelling(false);
    setScanError("");
    setScan(null);
    setPreview("选择文件可预览解析后的文本…");
    setDetail(null);
    setResults([]);
    setSummary(null);
    setProgress({});
    setSummaries({});
    setReviewTotal(0);
    setScanProgress({ task_id: requestId, phase: "discover", discovered: 0, processed: 0, total: null, matched: 0, current: "" });
    let unlisten: (() => void) | undefined;
    try {
      await api.saveSettings(appConfig);
      setConfigDirty(false);
      // 先完成订阅再调用后端，避免短任务漏掉首个事件。
      unlisten = await listen<ScanProgress>("scan://progress", ({ payload }) => {
        if (payload.task_id === activeScanRef.current) setScanProgress(payload);
      });
      let result: ScanResult;
      if (source === "dir") {
        if (!dirPath.trim()) throw new Error("请填写目录路径");
        result = await api.scanDirectory({
          requestId,
          path: dirPath,
          ruleSet,
          recursive,
          matchedOnly,
          threshold: appConfig.chunk_threshold,
          chunkSize: appConfig.chunk_size,
        });
      } else if (source === "upload") {
        if (uploadPaths.length === 0) throw new Error("请先选择文件或 ZIP");
        result = await api.scanUploads({
          requestId,
          paths: uploadPaths,
          ruleSet,
          matchedOnly,
          threshold: appConfig.chunk_threshold,
          chunkSize: appConfig.chunk_size,
        });
      } else {
        if (!textContent.trim()) throw new Error("请先粘贴内容");
        result = await api.scanText({
          requestId,
          content: textContent,
          system: textSystem,
          templateId: moduleTemplate(textSystem)?.id ?? "",
          ruleSet,
          threshold: appConfig.chunk_threshold,
          chunkSize: appConfig.chunk_size,
        });
      }
      activeReviewRef.current = null;
      setScan(result);
      setScanPage(0);
      setStep(2);
      toast(`扫描完成：命中 ${result.matched_files} / ${result.total_files} 个文件`);
    } catch (e) {
      setScanError(String(e));
      setStep(1);
    } finally {
      unlisten?.();
      activeScanRef.current = null;
      setScanning(false);
      setScanCancelling(false);
    }
  };

  const cancelScan = async () => {
    if (!activeScanRef.current) return;
    setScanCancelling(true);
    try { await api.cancelScan(activeScanRef.current); }
    catch (e) { setScanCancelling(false); toast(String(e)); }
  };

  const updateEntry = (path: string, patch: Partial<ScanEntry>) => {
    setScan((prev) =>
      prev ? { ...prev, entries: prev.entries.map((e) => (e.path === path ? { ...e, ...patch } : e)) } : prev
    );
  };

  const selectedEntries = useMemo(() => (scan ? scan.entries.filter((e) => e.selected && e.matched) : []), [scan]);

  const buildFileInput = (list: ScanEntry[]): ReviewFileInput[] =>
    list.map((e) => ({
      path: e.path,
      abs_path: e.abs_path,
      name: e.name,
      kind: e.kind,
      system: e.system,
      template_id: moduleTemplate(e.system)?.id ?? "",
      template_content: moduleTemplate(e.system)?.content ?? "",
      output_format: moduleTemplate(e.system)?.output_format ?? "standard",
    }));

  const runRequest = async (files: ReviewFileInput[]) => {
    if (!scan || !appConfig) throw new Error("任务未就绪");
    if (appConfig.granularity === "single" && new Set(files.map((f) => f.system)).size > 1) {
      throw new Error("合并评审仅支持同一模块的文件；多个模块请使用逐文件评审。");
    }
    await api.saveSettings(appConfig);
    setConfigDirty(false);
    analysisRevisionRef.current += 1;
    activeReviewRef.current = scan.task_id;
    setProgress({});
    setReviewTotal(appConfig.granularity === "single" ? 1 : files.length);
    setReviewStage("正在评审…");
    return api.runReview({
      task_id: scan.task_id,
      workspace: scan.workspace,
      files,
      cfg: gatewayCfg(),
      concurrency: appConfig.concurrency,
      chunk_threshold: appConfig.chunk_threshold,
      chunk_size: appConfig.chunk_size,
      granularity: appConfig.granularity,
      output_format: appConfig.output_format,
      retry_count: appConfig.retry_count,
    });
  };

  const persistTask = async (summary: ReviewSummary) => {
    if (!scan || !ruleSet) return;
    await api.saveTask({
      sourceType: scan.source_type,
      sourceRef: scan.source_ref,
      ruleSet: scan.rule_set,
      templates: scan.templates,
      summary,
    });
    refreshHistory();
  };

  const autoSummarize = async (list: FileResult[]) => {
    if (!appConfig || appConfig.granularity !== "per_file_system") return;
    const revision = analysisRevisionRef.current;
    setReviewStage("正在生成模块汇总…");
    const ids = Array.from(new Set(list.map((r) => r.system ?? "other")));
    for (const sys of ids) {
      const items = list.filter((r) => (r.system ?? "other") === sys);
      if (items.length === 0) continue;
      try {
        const text = await api.summarizeSystem({
          cfg: gatewayCfg(),
          systemName: systemName(sys),
          files: items.map((r) => ({ file: r.file, verdict: r.verdict, risks: r.risks })),
        });
        if (revision !== analysisRevisionRef.current) return;
        setSummaries((prev) => ({ ...prev, [sys]: text }));
      } catch {
        // 自动汇总失败不阻断主流程
      }
    }
  };

  const confirmAndStart = async () => {
    if (running || scanning) return;
    setConfirmOpen(false);
    if (!scan || !appConfig || !ruleSet) return;
    const files = buildFileInput(selectedEntries);
    if (files.length === 0) {
      toast("没有待分析的文件");
      return;
    }
    setStep(4);
    setRunning(true);
    setResults([]);
    setSummary(null);
    setSummaries({});
    setProgress({});
    try {
      const result = await runRequest(files);
      setSummary(result);
      setResults(result.results);
      setReviewStage("正在保存结果…");
      await persistTask(result);
      toast(result.cancelled ? "已取消，部分结果已保存" : "评审完成，已保存到本机历史");
      await autoSummarize(result.results);
    } catch (e) {
      toast(String(e));
    } finally {
      setRunning(false);
    }
  };

  const retryFailed = async () => {
    if (!scan || !appConfig) return;
    const failedPaths = new Set(results.filter((r) => r.error).map((r) => r.file));
    const entries =
      appConfig.granularity === "single"
        ? scan.entries.filter((e) => e.selected && e.matched)
        : scan.entries.filter((e) => e.selected && e.matched && failedPaths.has(e.path));
    const files = buildFileInput(entries);
    if (files.length === 0) {
      toast("没有可重试的失败项");
      return;
    }
    setRunning(true);
    setSummary(null);
    try {
      const result = await runRequest(files);
      const merged = [...results];
      for (const r of result.results) {
        const idx = merged.findIndex((m) => m.file === r.file);
        if (idx >= 0) merged[idx] = r;
        else merged.push(r);
      }
      const next = recomputeSummary(scan.task_id, merged, result.cancelled);
      setResults(merged);
      setSummary(next);
      await persistTask(next);
      setSummaries({});
      await autoSummarize(merged);
      toast(`已重试 ${files.length} 个失败项`);
    } catch (e) {
      toast(String(e));
    } finally {
      setRunning(false);
    }
  };

  const cancelReview = async () => {
    if (scan) await api.cancelReview(scan.task_id);
    toast("已请求取消，等待当前请求结束…");
  };

  const buildMarkdown = (list: FileResult[]) =>
    list
      .map(
        (r) =>
          `## ${r.file}\n\n- 模块：${systemName(r.system)}\n- 结论：${r.verdict}\n\n### 依据\n${r.basis
            .map((b) => `- ${b}`)
            .join("\n")}\n\n### 风险\n${r.risks.map((b) => `- ${b}`).join("\n")}\n\n### 整改项\n${r.remediations
            .map((b) => `- ${b}`)
            .join("\n")}\n\n### 引用\n${r.citations.map((c) => `> ${c.file} · ${c.locator} · ${c.snippet}`).join("\n")}\n`
      )
      .join("\n---\n\n");

  const buildHtml = (list: FileResult[]) => {
    const rows = list
      .map(
        (r) =>
          `<section><h2>${escapeHtml(r.file)}</h2><p class="meta">${escapeHtml(systemName(r.system))} · <b class="${verdictClass(
            r.verdict
          )}">${r.verdict}</b>${r.chunks > 1 ? ` · ${r.chunks} 块合并` : ""}</p>
           <h3>依据</h3><ul>${r.basis.map((b) => `<li>${escapeHtml(b)}</li>`).join("") || "<li>无</li>"}</ul>
           <h3>风险</h3><ul>${r.risks.map((b) => `<li>${escapeHtml(b)}</li>`).join("") || "<li>无</li>"}</ul>
           <h3>整改项</h3><ul>${r.remediations.map((b) => `<li>${escapeHtml(b)}</li>`).join("") || "<li>无</li>"}</ul>
           ${
             r.citations.length
               ? `<h3>引用</h3>${r.citations
                   .map((c) => `<blockquote>${escapeHtml(c.file)} · ${escapeHtml(c.locator)} · ${escapeHtml(c.snippet)}</blockquote>`)
                   .join("")}`
               : ""
           }</section>`
      )
      .join("");
    return `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>配置评审报告</title>
      <style>body{font:14px/1.7 -apple-system,"Microsoft YaHei UI",sans-serif;max-width:900px;margin:40px auto;padding:0 20px;color:#1c2431}
      h1{font-family:Georgia,serif}h2{font-family:Georgia,serif;border-bottom:1px solid #e3dccd;padding-bottom:6px;margin-top:32px}
      h3{margin:14px 0 6px;font-size:14px;color:#3d6240}.meta{color:#7d7666;font-size:12px}
      .pass{color:#3d6240}.conditional{color:#a2762a}.block,.fail{color:#8f3b34}
      blockquote{border-left:2px solid #b08d33;margin:4px 0;padding:4px 12px;color:#7d7666;font-family:monospace;font-size:12px}</style>
      </head><body><h1>配置评审报告</h1><p class="meta">生成时间：${formatTime(Date.now())}</p>${rows}</body></html>`;
  };

  const exportList = (list: FileResult[], format: "json" | "md" | "html") => {
    if (list.length === 0) return toast("没有可导出的结果");
    if (format === "json") download(`review-${Date.now()}.json`, JSON.stringify(list, null, 2), "application/json");
    if (format === "md") download(`review-${Date.now()}.md`, buildMarkdown(list), "text/markdown");
    if (format === "html") download(`review-${Date.now()}.html`, buildHtml(list), "text/html");
  };

  const exportExcelOf = async (list: FileResult[], ruleByPath: Map<string, string>) => {
    if (exporting) return;
    if (list.length === 0) return toast("没有可导出的结果");
    const path = await save({
      defaultPath: `review-${Date.now()}.xlsx`,
      filters: [{ name: "Excel", extensions: ["xlsx"] }],
    });
    if (!path) return;
    const entryByPath = new Map((scan?.entries ?? []).map((e) => [e.path, e]));
    const now = formatTime(Date.now());
    const rows: ExportRow[] = list.map((r) => {
      const entry = entryByPath.get(r.file);
      return {
        system: systemName(r.system),
        file: r.file,
        file_type: entry ? kindLabel(entry.kind) : "",
        rule: ruleByPath.get(r.file) ?? entry?.rule_id ?? "",
        verdict: r.verdict,
        citations: r.citations.map((c) => `${c.locator} ${c.snippet}`).join("；"),
        time: now,
      };
    });
    setExporting(true);
    try {
      await api.exportXlsx(path, rows);
      toast("已导出 Excel");
    } catch (e) {
      toast(String(e));
    } finally { setExporting(false); }
  };

  // 命中规则：rule_id → glob 模式，供导出展示。
  const ruleMap = useMemo(() => {
    const map = new Map<string, string>();
    const rs = scan?.rule_set;
    for (const e of scan?.entries ?? []) {
      if (!e.rule_id) continue;
      const rule = rs?.rules.find((x) => x.id === e.rule_id);
      map.set(e.path, rule ? rule.pattern : e.rule_id);
    }
    return map;
  }, [scan]);

  const exportExcel = () => exportExcelOf(results, ruleMap);

  const download = async (filename: string, content: string, _type: string) => {
    if (exporting) return;
    try {
      const extension = filename.split(".").pop() ?? "txt";
      const path = await save({ defaultPath: filename, filters: [{ name: extension.toUpperCase(), extensions: [extension] }] });
      if (!path) return;
      setExporting(true);
      await api.exportText(path, content);
      toast("报告已导出");
    } catch (e) { toast(String(e)); }
    finally { setExporting(false); }
  };

  const summarize = async (system: string) => {
    if (summarizing || running) return;
    const revision = analysisRevisionRef.current;
    const owner = activeReviewRef.current;
    const items = results.filter((r) => (r.system ?? "other") === system);
    if (items.length === 0) return;
    setSummarizing(system);
    const files: SystemFileSummary[] = items.map((r) => ({ file: r.file, verdict: r.verdict, risks: r.risks }));
    try {
      const text = await api.summarizeSystem({
        cfg: gatewayCfg(),
        systemName: systemName(system),
        files,
      });
      if (owner !== activeReviewRef.current || revision !== analysisRevisionRef.current) return;
      setSummaries((prev) => ({ ...prev, [system]: text }));
      toast(`已生成 ${systemName(system)} 汇总`);
    } catch (e) {
      toast(String(e));
    } finally { setSummarizing(null); }
  };

  const viewHistory = async (id: string) => {
    try {
      setHistoryDetail(await api.getHistory(id));
    } catch (e) {
      toast(String(e));
    }
  };

  const removeHistory = async (id: string) => {
    try {
      await api.deleteHistory(id);
      if (historyDetail?.summary.id === id) setHistoryDetail(null);
      refreshHistory();
    } catch (e) {
      toast(String(e));
    }
  };

  const reevalHistory = () => {
    if (!historyDetail) return;
    if (historyDetail.summary.source_type !== "dir") {
      toast("该来源为上传/文本，原始文件已清理，无法重评；请重新选择来源。");
      return;
    }
    setDirPath(historyDetail.summary.source_ref);
    setSource("dir");
    setHistoryDetail(null);
    setView("task");
    setStep(1);
    toast("已载入来源目录，将使用当前评审模块重新扫描。");
  };

  const saveSettings = async () => {
    if (!appConfig) return;
    try {
      await api.saveSettings(appConfig, apiKey || undefined);
      if (apiKey) {
        setHasKey(true);
        setApiKey("");
      }
      setConfigDirty(false);
      toast("设置已保存");
    } catch (e) {
      toast(String(e));
    }
  };

  const progressList = Object.entries(progress)
    .map(([k, v]) => ({ index: Number(k), ...v }))
    .sort((a, b) => a.index - b.index);
  const doneCount = progressList.filter((p) => p.phase === "done").length;

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">审</div>
          <div className="brand-copy">
            <strong>配置评审台</strong>
            <small>内网版 · 混合包评审</small>
          </div>
        </div>
        <div className="nav-label">工作区</div>
        <button className={`nav-item ${view === "task" ? "is-active" : ""}`} onClick={() => setView("task")}>
          <span className="ico">✎</span>新建评审
        </button>
        <button className={`nav-item ${view === "history" ? "is-active" : ""}`} onClick={() => setView("history")}>
          <span className="ico">≡</span>评审历史
          {history.length > 0 && <span className="badge">{history.length}</span>}
        </button>
        <button className={`nav-item ${view === "templates" ? "is-active" : ""}`} onClick={() => setView("templates")}>
          <span className="ico">▤</span>评审模块
        </button>
        <div className="nav-label">配置</div>
        <button className={`nav-item ${view === "settings" ? "is-active" : ""}`} onClick={() => setView("settings")}>
          <span className="ico">⚙</span>模型与运行
        </button>
        <div className="sidebar-foot">
          <div className="conn">
            <span className="dot" style={{ background: connectionResult ? (connectionResult.ok ? "var(--ok)" : "var(--danger)") : "var(--muted)" }} />
            <span>
              <strong>{appConfig?.base_url ? "模型服务" : "未配置"}</strong>
              {appConfig?.base_url ? (connectionResult ? (connectionResult.ok ? " · 连接正常" : " · 连接失败") : hasKey ? " · 已配置" : " · 缺 Key") : " · 请设置"}
            </span>
          </div>
          <div className="ver">{appVersion ? `v${appVersion}` : ""} · Tauri</div>
        </div>
      </aside>

      <div className="main">
        <header className="topbar">
          <div>
            <h1>
              {view === "task" && "新建评审"}
              {view === "history" && "评审历史"}
              {view === "templates" && "评审模块"}
              {view === "settings" && "模型与运行"}
            </h1>
            <div className="sub">
              {view === "task" && "上传混合发布包 → 识别模块 → 分模块评审"}
              {view === "history" && "本机任务与文件级结果"}
              {view === "templates" && "文件匹配 → 评审规范 → 模型输出"}
              {view === "settings" && "模型连接、凭据与运行容量"}
            </div>
          </div>
          <div className="topbar-actions">
            <span className="chip good">Private Banking</span>
          </div>
        </header>

        {(connectionTesting || exporting || summarizing) && <section className="scan-progress panel" role="status">
          <strong>{[connectionTesting ? "正在测试模型连接…" : "", exporting ? "正在导出报告…" : "", summarizing ? `正在生成 ${systemName(summarizing)} 汇总…` : ""].filter(Boolean).join(" · ")}</strong>
          <progress aria-label="操作进行中" />
        </section>}

        {(scanning || scanError) && (
          <section className="scan-progress panel" aria-label="扫描进度" aria-busy={scanning}>
            <div className="inline">
              <strong>{scanError || (scanCancelling ? "正在取消扫描…" : ({ discover: "正在发现文件…", extract: "正在解压 ZIP…", parse: "正在解析文档…", done: "正在整理扫描结果…" }[scanProgress?.phase ?? "discover"]))}</strong>
              <span className="spacer" />
              {scanning && <button className="btn btn-sm" disabled={scanCancelling} onClick={cancelScan}>{scanCancelling ? "取消中…" : "取消扫描"}</button>}
              {!scanning && <button className="btn btn-sm" onClick={() => setScanError("")}>关闭</button>}
            </div>
            {scanning && <>
              <progress aria-label="扫描解析进度" max={scanProgress?.total || 1} value={scanProgress?.total == null ? undefined : scanProgress.total === 0 ? 1 : scanProgress.processed} />
              <div className="muted" aria-live="polite">
                {scanProgress?.total == null ? `已发现 ${scanProgress?.discovered ?? 0} 个文件，正在统计总数` : `已处理 ${scanProgress.processed} / ${scanProgress.total} · 命中 ${scanProgress.matched} · ${scanProgress.total ? Math.round(scanProgress.processed / scanProgress.total * 100) : 100}%`}
              </div>
              <div className="scan-current" title={scanProgress?.current}>{scanProgress?.current || "准备工作区…"}</div>
              {scanCancelling && <small>将在当前文档解析返回后停止；ZIP 解压会在读取分段间停止。</small>}
            </>}
          </section>
        )}

        {view === "task" && !appConfig && <section className="view"><div className="empty">正在加载配置…</div></section>}
        {view === "task" && appConfig && (
          <section className="view">
            <div className="steps">
              {[
                ["1", "选择来源", "ZIP / 目录 / 文本"],
                ["2", "扫描与解析", "识别评审模块"],
                ["3", "确认评审", "确认范围与输出"],
                ["4", "分析结果", "按模块分组"],
              ].map(([num, title, sub]) => (
                <div
                  key={num}
                  className={`step ${step === Number(num) ? "is-active" : ""} ${step > Number(num) ? "is-done" : ""}`}
                  onClick={() => setStep(Number(num))}
                >
                  <span className="num">{num}</span>
                  <span className="txt">
                    <strong>{title}</strong>
                    <small>{sub}</small>
                  </span>
                </div>
              ))}
            </div>

            {step === 1 && (
              <div className="panel">
                <div className="panel-head">
                  <h2>选择评审来源</h2>
                  <span className="hint">文件数 &lt; {appConfig.max_files} · 单文件 ≤ {appConfig.max_file_mb} MB</span>
                </div>
                <div className="panel-body">
                  <div className="src-tabs">
                    <button className={source === "upload" ? "is-active" : ""} onClick={() => setSource("upload")}>
                      上传 ZIP / 文件
                    </button>
                    <button className={source === "dir" ? "is-active" : ""} onClick={() => setSource("dir")}>
                      扫描本地目录
                    </button>
                    <button className={source === "text" ? "is-active" : ""} onClick={() => setSource("text")}>
                      粘贴文本
                    </button>
                  </div>

                  {source === "upload" && (
                    <>
                      <div className="dropzone" onClick={pickFiles}>
                        <strong>选择发布包（ZIP）或多个文件</strong>
                        <small>包内可含 Apollo JSON、XXL-JOB Excel/Word、星云配置文件夹等</small>
                      </div>
                      <div className="inline" style={{ marginTop: 10 }}>
                        <button className="btn btn-block" onClick={pickFiles}>
                          选择文件
                        </button>
                      </div>
                      {uploadPaths.length > 0 && <div className="preview-box" style={{ maxHeight: 120 }}>{uploadPaths.join("\n")}</div>}
                    </>
                  )}

                  {source === "dir" && (
                    <>
                      <div className="field">
                        <label>目录路径</label>
                        <div className="inline">
                          <input value={dirPath} onChange={(e) => setDirPath(e.target.value)} placeholder={"D:\\release\\2026Q3-mixed"} />
                          <button className="btn" onClick={pickDir}>
                            浏览…
                          </button>
                        </div>
                      </div>
                      <label className="checkline">
                        <input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} /> 递归扫描子目录
                      </label>
                    </>
                  )}

                  {source === "text" && (
                    <>
                      <div className="field">
                        <label>粘贴要评审的内容</label>
                        <textarea value={textContent} onChange={(e) => setTextContent(e.target.value)} placeholder="把配置或文档内容粘贴到这里…" />
                      </div>
                      <div className="field-row">
                        <div className="field">
                          <label>评审模块</label>
                          <select value={textSystem} onChange={(e) => setTextSystem(e.target.value)}>
                            {systems.map((s) => (
                              <option key={s.id} value={s.id}>
                                {s.name}
                              </option>
                            ))}
                          </select>
                        </div>
                      </div>
                    </>
                  )}

                  <div className="field" style={{ marginTop: 16 }}>
                    {appConfig.rule_sets.length > 1 && <><label htmlFor="source-rule-set">文件匹配方案（兼容旧配置）</label>
                      <select id="source-rule-set" value={ruleSet?.id ?? ""} disabled={scanning || running} onChange={(e) => changeRuleSet(e.target.value)}>
                        {appConfig.rule_sets.map((rs) => <option key={rs.id} value={rs.id}>{rs.name}</option>)}
                      </select></>}
                    <div className="inline"><span>已配置 {systems.length} 个评审模块</span>
                      <button className="btn" onClick={() => setView("templates")}>管理评审模块</button></div>
                    <small>按模块的文件匹配条件自动归属，使用该模块的评审规范。支持 TXT、JSON、Excel、DOC / DOCX；PDF 暂不支持解析。</small>
                  </div>

                  <label className="checkline">
                    <input type="checkbox" checked={matchedOnly} onChange={(e) => setMatchedOnly(e.target.checked)} /> 仅保留命中规则的文件
                  </label>

                  <div className="step-foot">
                    <span className="spacer" />
                    <button className="btn btn-primary btn-lg" onClick={doScan} disabled={scanning || running}>
                      {scanning ? "扫描中…" : "下一步：扫描解析 →"}
                    </button>
                  </div>
                </div>
              </div>
            )}

            {step === 2 && scan && (
              <div className="panel">
                <div className="panel-head">
                  <h2>扫描与解析</h2>
                  <span className="hint">文件自动归入模块，使用模块的评审规范</span>
                  <div className="right">
                    <button className="btn btn-sm" onClick={doScan} disabled={scanning || running}>
                      重新扫描
                    </button>
                  </div>
                </div>
                <div className="panel-body">
                  <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                    <span className="tag">匹配方案：{scan.rule_set.name}</span>
                    <span className="muted" style={{ marginLeft: "auto", fontSize: 11.5 }}>
                      命中 {scan.matched_files} / {scan.total_files}
                    </span>
                  </div>
                  <div className="table-scroll">
                    <table className="data">
                      <thead>
                        <tr>
                          <th style={{ width: 34 }}>分析</th>
                          <th>文件</th>
                          <th style={{ width: 70 }}>类型</th>
                          <th style={{ width: 80 }}>大小</th>
                          <th style={{ width: 100 }}>评审模块</th>
                          <th style={{ width: 200 }}>输出格式</th>
                          <th style={{ width: 70 }}>分块</th>
                          <th style={{ width: 60 }}></th>
                        </tr>
                      </thead>
                      <tbody>
                        {scan.entries.slice(scanPage * 100, (scanPage + 1) * 100).map((e) => (
                          <tr key={e.path}>
                            <td>
                              <input
                                type="checkbox"
                                checked={e.selected}
                                disabled={!e.matched || e.parse_status === "error"}
                                onChange={(ev) => updateEntry(e.path, { selected: ev.target.checked })}
                              />
                            </td>
                            <td>
                              <span className="fname">{e.name}</span>
                              <br />
                              <span className="path">{e.path}</span>
                            </td>
                            <td>
                              <span className={`tag ${e.kind}`}>{kindLabel(e.kind)}</span>
                            </td>
                            <td>{(e.size / 1024).toFixed(1)} KB</td>
                            <td>{e.matched ? <span className={`sys ${systemClass(e.system)}`} style={sysStyle(e.system)}>{systemName(e.system)}</span> : <span className="tag skip">未命中</span>}</td>
                            <td>
                              {e.matched ? <span>{OUTPUT_FORMAT_LABEL[moduleTemplate(e.system)?.output_format ?? "standard"]}</span> : <span className="tag skip">跳过</span>}
                            </td>
                            <td>
                              {e.matched ? (
                                e.parse_status === "error" ? (
                                  <span className="verdict fail">解析失败</span>
                                ) : e.chunks > 1 ? (
                                  <span className="verdict conditional">{e.chunks} 块</span>
                                ) : (
                                  "1 块"
                                )
                              ) : (
                                "—"
                              )}
                            </td>
                            <td>
                              {e.matched && (
                                <button
                                  className="link"
                                  onClick={() =>
                                    setPreview(
                                      `${e.name} · ${systemName(e.system)}\n${"─".repeat(40)}\n${e.preview || e.parse_error || "（无预览）"}`
                                    )
                                  }
                                >
                                  预览
                                </button>
                              )}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                  {scan.entries.length > 100 && <div className="inline pagination">
                    <button className="btn btn-sm" disabled={scanPage === 0} onClick={() => setScanPage((p) => p - 1)}>上一页</button>
                    <span>第 {scanPage + 1} / {Math.ceil(scan.entries.length / 100)} 页 · 共 {scan.entries.length} 个文件</span>
                    <button className="btn btn-sm" disabled={(scanPage + 1) * 100 >= scan.entries.length} onClick={() => setScanPage((p) => p + 1)}>下一页</button>
                  </div>}

                  <div className="preview-box">{preview}</div>
                  <div className="step-foot">
                    <button className="btn" onClick={() => setStep(1)}>
                      ← 上一步
                    </button>
                    <span className="spacer" />
                    <button className="btn btn-primary btn-lg" onClick={() => setStep(3)}>
                      下一步：评审配置 →
                    </button>
                  </div>
                </div>
              </div>
            )}

            {step === 3 && (
              <div className="panel">
                <div className="panel-head">
                  <h2>确认评审</h2>
                  <span className="hint">输入、评审要求和输出格式统一来自评审模块</span>
                </div>
                <div className="panel-body">
                  <div className="field-row">
                    <div className="field">
                      <label>模块输出格式</label>
                      <p>{selectedFormats.join(" / ") || "请先扫描并选择文件"}</p>
                      <small>由各模块的评审规范决定，可在「评审模块」中修改。</small>
                    </div>
                    <div className="field">
                      <label>分析粒度</label>
                      <select
                        value={appConfig.granularity}
                        onChange={(e) => updateConfig({ granularity: e.target.value as Granularity })}
                      >
                        {(Object.keys(GRANULARITY_LABEL) as Granularity[]).map((g) => (
                          <option key={g} value={g}>
                            {GRANULARITY_LABEL[g]}
                          </option>
                        ))}
                      </select>
                    </div>
                  </div>
                  <details className="runtime-options">
                    <summary>高级运行设置（通常无需修改）</summary>
                  <div className="field-row">
                    <div className="field">
                      <label>并发数</label>
                      <input type="number" min={1} max={8} value={appConfig.concurrency} onChange={(e) => updateConfig({ concurrency: Number(e.target.value) || 1 })} />
                    </div>
                    <div className="field">
                      <label>失败自动重试次数</label>
                      <input type="number" min={0} max={5} value={appConfig.retry_count} onChange={(e) => updateConfig({ retry_count: Number(e.target.value) || 0 })} />
                    </div>
                  </div>
                  <div className="field-row">
                    <div className="field">
                      <label>分块触发阈值（字符）</label>
                      <input type="number" value={appConfig.chunk_threshold} onChange={(e) => updateConfig({ chunk_threshold: Number(e.target.value) || 12000 })} />
                    </div>
                    <div className="field">
                      <label>单块大小（字符）</label>
                      <input type="number" value={appConfig.chunk_size} onChange={(e) => updateConfig({ chunk_size: Number(e.target.value) || 8000 })} />
                    </div>
                  </div>
                  </details>
                  <p className="muted" style={{ fontSize: 12 }}>
                    {appConfig.granularity === "single"
                      ? "将同一模块内的所选文件合并评审，核对文件间的一致性。"
                      : `将分析 ${selectedEntries.length} 个文件${appConfig.granularity === "per_file_system" ? "，并按模块自动生成汇总" : ""}；失败单文件自动隔离并计入失败，不影响其他文件。`}
                  </p>
                  {mixedModuleMerge && <p className="error-text" role="alert">所选文件属于多个模块，请改为逐文件评审，或返回清单只选择同一模块的文件。</p>}
                  <div className="step-foot">
                    <button className="btn" onClick={() => setStep(2)}>
                      ← 上一步
                    </button>
                    <span className="spacer" />
                    {running ? (
                      <button className="btn btn-primary btn-lg" onClick={cancelReview}>
                        取消评审
                      </button>
                    ) : (
                      <button className="btn btn-primary btn-lg" onClick={() => setConfirmOpen(true)} disabled={selectedEntries.length === 0 || mixedModuleMerge}>
                        开始评审
                      </button>
                    )}
                  </div>
                </div>
              </div>
            )}

            {step === 4 && (
              <>
                <div className="panel" style={{ marginBottom: 15 }}>
                  <div style={{ padding: 17 }}>
                    <div style={{ display: "flex", alignItems: "center", marginBottom: 10 }}>
                      <strong style={{ fontFamily: "var(--heading-font)", fontStyle: "italic" }}>
                        {running ? reviewStage : summary?.cancelled ? "已取消" : summary ? "评审完成" : "等待开始评审"}
                      </strong>
                      <span className="muted" style={{ marginLeft: "auto", fontFamily: "var(--mono)", fontSize: 12 }}>
                        {Math.min(doneCount, reviewTotal)} / {reviewTotal}
                      </span>
                      {running && <button className="btn btn-sm" style={{ marginLeft: 12 }} onClick={cancelReview}>取消评审</button>}
                    </div>
                    <progress aria-label="评审完成进度" max={reviewTotal || 1} value={Math.min(doneCount, reviewTotal)} />
                    {running && <div className="work-pulse" aria-label="评审运行中" />}
                    {running && <small className="muted">进度按已完成评审单元统计；单个文件或合并评审时，模型返回前保持等待动画。</small>}
                    <div className="summary">
                      <div className="stat good">
                        <strong>{summary?.pass ?? 0}</strong>
                        <small>通过</small>
                      </div>
                      <div className="stat warn">
                        <strong>{summary?.conditional ?? 0}</strong>
                        <small>有条件通过</small>
                      </div>
                      <div className="stat danger">
                        <strong>{summary?.block ?? 0}</strong>
                        <small>阻断</small>
                      </div>
                      <div className="stat">
                        <strong>{summary?.fail ?? 0}</strong>
                        <small>失败</small>
                      </div>
                    </div>
                  </div>
                </div>

                <div className="result-grid">
                  <div>
                    {groupSystems.map((sys) => {
                      const items = results.filter((r) => (r.system ?? "other") === sys);
                      if (items.length === 0) return null;
                      return (
                        <div className="panel" key={sys} style={{ marginBottom: 15 }}>
                          <div className="group-head">
                            <span className={`sys ${sys}`} style={sysStyle(sys)}>{systemName(sys)}</span>
                            <h3>{items.length} 个文件</h3>
                            <div className="right">
                              <button className="link" disabled={running || summarizing !== null} onClick={() => summarize(sys)}>
                                生成模块汇总
                              </button>
                            </div>
                          </div>
                          <table className="data">
                            <thead>
                              <tr>
                                <th>文件</th>
                                <th style={{ width: 110 }}>结论</th>
                                <th style={{ width: 70 }}>耗时</th>
                                <th style={{ width: 56 }}></th>
                              </tr>
                            </thead>
                            <tbody>
                              {items.map((r) => (
                                <tr key={r.file}>
                                  <td>
                                    <span className="fname">{r.file}</span>
                                    {r.chunks > 1 && <span className="tag"> {r.chunks} 块合并</span>}
                                  </td>
                                  <td>
                                    <span className={`verdict ${verdictClass(r.verdict)}`}>{r.verdict}</span>
                                  </td>
                                  <td>{(r.duration_ms / 1000).toFixed(1)}s</td>
                                  <td>
                                    <button className="link" onClick={() => setDetail(r)}>
                                      详情
                                    </button>
                                  </td>
                                </tr>
                              ))}
                            </tbody>
                          </table>
                          {summaries[sys] && (
                            <div className="detail-body" style={{ borderTop: "1px solid var(--line)", whiteSpace: "pre-wrap" }}>
                              <span className="k">模块汇总：</span>
                              {"\n"}
                              {summaries[sys]}
                            </div>
                          )}
                        </div>
                      );
                    })}
                    {running && (
                      <div className="panel">
                        <div className="panel-head">
                          <h2>实时输出</h2>
                        </div>
                        <div className="panel-body">
                          {progressList.length === 0 && <div className="empty">等待模型返回…</div>}
                          {progressList.map((p) => (
                            <div key={p.index} style={{ marginBottom: 10 }}>
                              <div className="muted" style={{ fontSize: 12 }}>
                                {p.file} · {({ parse: "解析资料", waiting: "等待模型", analyze: "生成评审", chunk: "分块评审", merge: "合并结果", done: "已完成" } as Record<string, string>)[p.phase] ?? "评审中"}
                              </div>
                              <div className="stream-box">{p.text || "…"}</div>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>

                  <div className="panel">
                    <div className="panel-head">
                      <h2>结果详情</h2>
                      <span className="hint">{detail?.file ?? "—"}</span>
                    </div>
                    <div className="detail-body">
                      {!detail && <p className="muted">点击文件查看结论、依据、风险与整改项。</p>}
                      {detail && (
                        <>
                          <p>
                            <span className="k">模块：</span>
                            {systemName(detail.system)}
                            {detail.chunks > 1 ? ` · 基于 ${detail.chunks} 块合并` : ""}
                          </p>
                          <p>
                            <span className="k">结论：</span>
                            {detail.verdict}
                          </p>
                          <p>
                            <span className="k">依据：</span>
                          </p>
                          <ul>{(detail.basis.length ? detail.basis : ["无"]).map((x, i) => <li key={i}>{x}</li>)}</ul>
                          <p>
                            <span className="k">风险：</span>
                          </p>
                          <ul>{(detail.risks.length ? detail.risks : ["无"]).map((x, i) => <li key={i}>{x}</li>)}</ul>
                          <p>
                            <span className="k">整改项：</span>
                          </p>
                          <ul>{(detail.remediations.length ? detail.remediations : ["无"]).map((x, i) => <li key={i}>{x}</li>)}</ul>
                          {detail.error && <p className="error-text">失败原因：{detail.error}</p>}
                          <p>
                            <span className="k">引用：</span>
                          </p>
                          {detail.citations.length === 0 && <p className="muted">无</p>}
                          {detail.citations.map((c, i) => (
                            <blockquote key={i}>
                              {c.file} · {c.locator} · {c.snippet}
                            </blockquote>
                          ))}
                        </>
                      )}
                    </div>
                    <div className="panel-head" style={{ borderTop: "1px solid var(--line)", borderBottom: 0 }}>
                      <h2>导出</h2>
                    </div>
                    <div className="panel-body export-row">
                      <button className="btn" onClick={exportExcel} disabled={results.length === 0}>
                        导出 Excel
                      </button>
                      <button className="btn" onClick={() => exportList(results, "json")} disabled={results.length === 0}>
                        导出 JSON
                      </button>
                      <button className="btn" onClick={() => exportList(results, "md")} disabled={results.length === 0}>
                        导出 Markdown
                      </button>
                      <button className="btn" onClick={() => exportList(results, "html")} disabled={results.length === 0}>
                        导出 HTML
                      </button>
                    </div>
                  </div>
                </div>

                <div className="step-foot">
                  <button className="btn" onClick={() => setStep(3)}>
                    ← 修改配置
                  </button>
                  {(summary?.fail ?? 0) > 0 && !running && (
                    <button className="btn btn-ghost" onClick={retryFailed}>
                      重试失败项（{summary?.fail}）
                    </button>
                  )}
                  <span className="spacer" />
                  <button className="btn btn-ghost" onClick={() => setView("history")}>
                    查看历史
                  </button>
                </div>
              </>
            )}
          </section>
        )}

        {view === "history" && (
          <section className="view">
            <div className="panel">
              <div className="panel-head">
                <h2>评审历史</h2>
                <div className="right">
                  <input
                    className="search"
                    placeholder="搜索来源 / 模块"
                    value={historySearch}
                    onChange={(e) => setHistorySearch(e.target.value)}
                  />
                </div>
              </div>
              <table className="data">
                <thead>
                  <tr>
                    <th>时间</th>
                    <th>来源</th>
                    <th>含模块</th>
                    <th>文件数</th>
                    <th>结论分布</th>
                    <th style={{ width: 150 }}>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredHistory.length === 0 && (
                    <tr>
                      <td colSpan={6} className="empty">
                        {history.length === 0 ? "暂无历史。成功评审才会入库。" : "没有匹配的记录。"}
                      </td>
                    </tr>
                  )}
                  {filteredHistory.map((h) => (
                    <tr key={h.id}>
                      <td>{formatTime(h.created_at)}</td>
                      <td>
                        {h.source_type === "dir" ? "目录 · " : h.source_type === "text" ? "文本 · " : "文件 · "}
                        {h.source_ref}
                      </td>
                      <td>
                        {h.systems.map((s) => (
                          <span key={s} className={`sys ${s}`} style={{ marginRight: 8, ...sysStyle(s) }}>
                            {systemName(s)}
                          </span>
                        ))}
                      </td>
                      <td>{h.total_files}</td>
                      <td>
                        <span className="verdict pass">通过 {h.pass}</span>{" "}
                        <span className="verdict conditional">有条件 {h.conditional}</span>{" "}
                        <span className="verdict block">阻断 {h.block}</span>
                        {h.fail > 0 && <span className="verdict fail"> 失败 {h.fail}</span>}
                      </td>
                      <td>
                        <div style={{ display: "flex", gap: 12 }}>
                          <button className="link" onClick={() => viewHistory(h.id)}>
                            查看
                          </button>
                          <button className="link danger" onClick={() => removeHistory(h.id)}>
                            删除
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {historyDetail && (
              <div className="panel" style={{ marginTop: 15 }}>
                <div className="panel-head">
                  <h2>任务详情 · {historyDetail.summary.source_ref}</h2>
                  <div className="right">
                    <button className="btn btn-sm" onClick={() => exportExcelOf(historyDetail.files, new Map())}>
                      导出 Excel
                    </button>
                    <button className="btn btn-sm" onClick={() => exportList(historyDetail.files, "json")}>
                      JSON
                    </button>
                    <button className="btn btn-sm" onClick={() => exportList(historyDetail.files, "md")}>
                      Markdown
                    </button>
                    <button className="btn btn-sm" onClick={() => exportList(historyDetail.files, "html")}>
                      HTML
                    </button>
                    <button className="btn btn-sm" onClick={reevalHistory}>
                      重评
                    </button>
                    <button className="btn btn-sm" onClick={() => setHistoryDetail(null)}>
                      关闭
                    </button>
                  </div>
                </div>
                <table className="data">
                  <thead>
                    <tr>
                      <th>文件</th>
                      <th style={{ width: 90 }}>模块</th>
                      <th style={{ width: 110 }}>结论</th>
                    </tr>
                  </thead>
                  <tbody>
                    {historyDetail.files.map((f) => (
                      <tr key={f.file}>
                        <td>{f.file}</td>
                        <td>
                          <span className={`sys ${systemClass(f.system)}`} style={sysStyle(f.system)}>{systemName(f.system)}</span>
                        </td>
                        <td>
                          <span className={`verdict ${verdictClass(f.verdict)}`}>{f.verdict}</span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        )}

        {view === "templates" && appConfig && (
          <ReviewModules config={appConfig} defaults={defaults} activeRuleSetId={activeRuleSetId}
            onRuleSetChange={changeRuleSet} onChange={updateConfig} disabled={running || scanning}
            dirty={configDirty} onSave={async () => { await api.saveSettings(appConfig); setConfigDirty(false); }} />
        )}

        {view === "settings" && appConfig && (
          <section className="view">
            <div className="two-col settings-layout">
              <div className="panel">
                <div className="panel-head">
                  <h2>模型与连接</h2>
                  <span className="hint">模型由使用者自行配置</span>
                </div>
                <div className="panel-body">
                  <div className="field">
                    <label htmlFor="setting-1">Base URL</label>
                    <input id="setting-1" value={appConfig.base_url} placeholder="http://llm-gateway.intra.corp/v1" onChange={(e) => updateConfig({ base_url: e.target.value })} />
                  </div>
                  <div className="field-row">
                    <div className="field">
                      <label htmlFor="setting-2">模型标识</label>
                      <input id="setting-2" value={appConfig.model} placeholder="例如 deepseek-v4-flash" onChange={(e) => updateConfig({ model: e.target.value })} />
                    </div>
                    <div className="field">
                      <label htmlFor="setting-3">并发数</label>
                      <input id="setting-3" type="number" min={1} max={8} value={appConfig.concurrency} onChange={(e) => updateConfig({ concurrency: Number(e.target.value) || 1 })} />
                    </div>
                  </div>
                  <div className="field">
                    <label htmlFor="setting-4">API Key（{hasKey ? "已保存，留空则不修改" : "未配置"}）</label>
                    <input id="setting-4" type="password" value={apiKey} placeholder={hasKey ? "••••••••" : "sk-…"} onChange={(e) => { setApiKey(e.target.value); setConnectionResult(null); }} />
                  </div>
                  <div className="field-row">
                    <div className="field">
                      <label htmlFor="setting-5">读取超时（秒）</label>
                      <input id="setting-5" type="number" value={appConfig.timeout_secs} onChange={(e) => updateConfig({ timeout_secs: Number(e.target.value) || 180 })} />
                    </div>
                    <div className="field">
                      <label htmlFor="setting-6">单文件上限（MB）</label>
                      <input id="setting-6" type="number" value={appConfig.max_file_mb} onChange={(e) => updateConfig({ max_file_mb: Number(e.target.value) || 1228 })} />
                    </div>
                  </div>
                  <div className="field-row">
                    <div className="field">
                      <label htmlFor="setting-7">最大文件数</label>
                      <input id="setting-7" type="number" value={appConfig.max_files} onChange={(e) => updateConfig({ max_files: Number(e.target.value) || 800 })} />
                    </div>
                    <div className="field">
                      <label htmlFor="setting-8">历史保留（天，0=永久）</label>
                      <input id="setting-8" type="number" value={appConfig.history_retention_days} onChange={(e) => updateConfig({ history_retention_days: Number(e.target.value) || 0 })} />
                    </div>
                  </div>
                  <div className="field">
                    <label htmlFor="setting-9">API Key 请求头（留空为 Bearer）</label>
                    <input id="setting-9" value={appConfig.api_key_header} placeholder="留空，或 api-key / X-API-Key" onChange={(e) => updateConfig({ api_key_header: e.target.value })} />
                  </div>
                  <div className="field">
                    <label htmlFor="setting-10">代理地址（留空直连内网）</label>
                    <input id="setting-10" value={appConfig.proxy_url} placeholder="可选：http://proxy.intra:8080" onChange={(e) => updateConfig({ proxy_url: e.target.value })} />
                  </div>
                  <label className="checkline">
                    <input type="checkbox" checked={appConfig.stream_enabled} onChange={(e) => updateConfig({ stream_enabled: e.target.checked })} /> 使用流式响应（网关不支持时关闭）
                  </label>
                  <label className="checkline">
                    <input type="checkbox" checked={appConfig.allow_http} onChange={(e) => updateConfig({ allow_http: e.target.checked })} /> 允许内网 HTTP（无证书网关）
                  </label>
                  <div style={{ display: "flex", gap: 9, marginTop: 14 }}>
                    <button
                      className="btn"
                      disabled={connectionTesting}
                      onClick={async () => {
                        setConnectionTesting(true);
                        setConnectionResult(null);
                        try {
                          const reply = await api.testConnection(gatewayCfg(), apiKey || undefined);
                          setConnectionResult({ ok: true, message: `连接正常：${reply}` });
                        } catch (e) {
                          setConnectionResult({ ok: false, message: String(e) });
                        } finally { setConnectionTesting(false); }
                      }}
                    >
                      {connectionTesting ? "连接测试中…" : "测试连接"}
                    </button>
                    <button className="btn btn-primary" onClick={saveSettings}>
                      保存设置
                    </button>
                  </div>
                  {connectionResult && <div className={`connection-result ${connectionResult.ok ? "success" : "error"}`} role={connectionResult.ok ? "status" : "alert"}>{connectionResult.message}</div>}
                  <p className="muted" style={{ fontSize: 11.5, marginTop: 10 }}>
                    API Key 通过系统凭据库加密保存（Windows 凭据管理器 / macOS Keychain），不进入配置、历史或导出。
                  </p>
                </div>
              </div>

              <div className="panel">
                <div className="panel-head"><h2>文件匹配与评审规范</h2></div>
                <div className="panel-body"><p>每个评审模块统一管理文件匹配、输入说明、评审要求和输出格式。</p>
                  <button className="btn" onClick={() => setView("templates")}>管理评审模块</button>
                </div>
              </div>
            </div>
          </section>
        )}
      </div>

      {confirmOpen && scan && appConfig && (
        <div className="modal-mask" onClick={() => setConfirmOpen(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <h2>确认数据去向</h2>
            </div>
            <div className="modal-body">
              <p className="muted" style={{ marginTop: 0 }}>
                即将把所选文件内容发送至下列模型服务进行评审，请确认。
              </p>
              <table className="data">
                <tbody>
                  <tr>
                    <td className="k">网关</td>
                    <td>{appConfig.base_url || "未配置"}</td>
                  </tr>
                  <tr>
                    <td className="k">模型</td>
                    <td>{appConfig.model || "未配置"}</td>
                  </tr>
                  <tr>
                    <td className="k">分析对象</td>
                    <td>
                      {appConfig.granularity === "single"
                        ? `${selectedEntries.length} 个文件（同模块合并评审）`
                        : `${selectedEntries.length} 个文件`}
                    </td>
                  </tr>
                  <tr>
                    <td className="k">输出格式 / 粒度</td>
                    <td>
                      {selectedFormats.join(" / ")} · {GRANULARITY_LABEL[appConfig.granularity]}
                    </td>
                  </tr>
                </tbody>
              </table>
              {!hasKey && <p className="error-text">尚未配置 API Key，请先到「模型与运行」设置。</p>}
            </div>
            <div className="modal-foot">
              <button className="btn" onClick={() => setConfirmOpen(false)}>
                取消
              </button>
              <span className="spacer" />
              <button className="btn btn-primary" onClick={confirmAndStart} disabled={!hasKey}>
                确认并开始
              </button>
            </div>
          </div>
        </div>
      )}

      {notice && <div className="toast is-visible">{notice}</div>}
    </div>
  );
}

function escapeHtml(text: string) {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
