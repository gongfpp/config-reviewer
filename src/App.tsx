import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";
import * as api from "./api";
import type {
  Defaults,
  FileResult,
  GatewayConfig,
  ReviewFileInput,
  ReviewProgress,
  ReviewSummary,
  RuleSet,
  ScanEntry,
  ScanResult,
  TaskDetail,
  TaskSummary,
  Template,
} from "./types";

type View = "task" | "history" | "templates" | "settings";

const SETTINGS_KEY = "reviewer.settings";

function loadSettings(): GatewayConfig {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (raw) return { allow_http: true, timeout_secs: 180, ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return { base_url: "", model: "", api_key: "", allow_http: true, timeout_secs: 180 };
}

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
  const [defaults, setDefaults] = useState<Defaults | null>(null);
  const [view, setView] = useState<View>("task");
  const [ruleSet, setRuleSet] = useState<RuleSet | null>(null);

  const [source, setSource] = useState<"upload" | "dir" | "text">("upload");
  const [uploadPaths, setUploadPaths] = useState<string[]>([]);
  const [dirPath, setDirPath] = useState("");
  const [recursive, setRecursive] = useState(true);
  const [matchedOnly, setMatchedOnly] = useState(true);

  const [step, setStep] = useState(1);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [scanning, setScanning] = useState(false);
  const [preview, setPreview] = useState("选择文件可预览解析后的文本…");

  const [cfg, setCfg] = useState<GatewayConfig>(loadSettings);
  const [concurrency, setConcurrency] = useState(3);
  const [threshold, setThreshold] = useState(12000);
  const [chunkSize, setChunkSize] = useState(8000);

  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Record<number, { file: string; phase: string; text: string }>>({});
  const [results, setResults] = useState<FileResult[]>([]);
  const [summary, setSummary] = useState<ReviewSummary | null>(null);
  const [detail, setDetail] = useState<FileResult | null>(null);
  const [notice, setNotice] = useState("");

  const [history, setHistory] = useState<TaskSummary[]>([]);
  const [historyDetail, setHistoryDetail] = useState<TaskDetail | null>(null);

  const [editingTemplate, setEditingTemplate] = useState<Template | null>(null);
  const noticeTimer = useRef<number | null>(null);

  const toast = useCallback((message: string) => {
    setNotice(message);
    if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(""), 2200);
  }, []);

  useEffect(() => {
    api
      .getDefaults()
      .then((d) => {
        setDefaults(d);
        setRuleSet(d.rule_sets[0] ?? null);
        setEditingTemplate(d.templates[0] ?? null);
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

  const systems = defaults?.systems ?? [];
  const templates = defaults?.templates ?? [];
  const systemName = (id: string | null) => systems.find((s) => s.id === id)?.name ?? id ?? "—";
  const systemClass = (id: string | null) => id ?? "other";

  const saveSettings = (next: GatewayConfig) => {
    setCfg(next);
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(next));
  };

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
    if (!ruleSet) return;
    setScanning(true);
    try {
      let result: ScanResult;
      if (source === "dir") {
        if (!dirPath.trim()) throw new Error("请填写目录路径");
        result = await api.scanDirectory({
          path: dirPath,
          ruleSet,
          recursive,
          matchedOnly,
          threshold,
          chunkSize,
        });
      } else if (source === "upload") {
        if (uploadPaths.length === 0) throw new Error("请先选择文件或 ZIP");
        result = await api.scanUploads({ paths: uploadPaths, ruleSet, matchedOnly, threshold, chunkSize });
      } else {
        throw new Error("粘贴文本模式将在后续版本接入");
      }
      setScan(result);
      setStep(2);
      toast(`扫描完成：命中 ${result.matched_files} / ${result.total_files} 个文件`);
    } catch (e) {
      toast(String(e));
    } finally {
      setScanning(false);
    }
  };

  const updateEntry = (path: string, patch: Partial<ScanEntry>) => {
    setScan((prev) =>
      prev
        ? { ...prev, entries: prev.entries.map((e) => (e.path === path ? { ...e, ...patch } : e)) }
        : prev
    );
  };

  const selectedEntries = useMemo(() => (scan ? scan.entries.filter((e) => e.selected && e.matched) : []), [scan]);

  const startReview = async () => {
    if (!scan || !ruleSet) return;
    const files: ReviewFileInput[] = selectedEntries.map((e) => ({
      path: e.path,
      abs_path: e.abs_path,
      name: e.name,
      kind: e.kind,
      system: e.system,
      template_id: e.template_id ?? "tpl-generic",
      template_content: templates.find((t) => t.id === e.template_id)?.content ?? "",
    }));
    if (files.length === 0) {
      toast("没有待分析的文件");
      return;
    }
    setStep(4);
    setRunning(true);
    setResults([]);
    setSummary(null);
    setProgress({});
    try {
      const result = await api.runReview({ files, cfg, concurrency, chunk_threshold: threshold, chunk_size: chunkSize });
      setSummary(result);
      setResults(result.results);
      await api.saveTask({
        sourceType: scan.source_type,
        sourceRef: scan.source_ref,
        ruleSet,
        templates,
        summary: result,
      });
      refreshHistory();
      toast("评审完成，已保存到本机历史");
    } catch (e) {
      toast(String(e));
    } finally {
      setRunning(false);
    }
  };

  const exportCurrent = (format: "json" | "markdown") => {
    const rows = results;
    if (rows.length === 0) {
      toast("没有可导出的结果");
      return;
    }
    let content = "";
    let filename = `review-${Date.now()}`;
    if (format === "json") {
      content = JSON.stringify(rows, null, 2);
      filename += ".json";
    } else {
      content = rows
        .map(
          (r) =>
            `## ${r.file}\n\n- 系统：${systemName(r.system)}\n- 结论：${r.verdict}\n\n### 依据\n${r.basis
              .map((b) => `- ${b}`)
              .join("\n")}\n\n### 风险\n${r.risks.map((b) => `- ${b}`).join("\n")}\n\n### 整改项\n${r.remediations
              .map((b) => `- ${b}`)
              .join("\n")}\n\n### 引用\n${r.citations
              .map((c) => `> ${c.file} · ${c.locator} · ${c.snippet}`)
              .join("\n")}\n`
        )
        .join("\n---\n\n");
      filename += ".md";
    }
    const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
  };

  const viewHistory = async (id: string) => {
    try {
      const d = await api.getHistory(id);
      setHistoryDetail(d);
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
          <span className="ico">▤</span>模板管理
        </button>
        <div className="nav-label">配置</div>
        <button className={`nav-item ${view === "settings" ? "is-active" : ""}`} onClick={() => setView("settings")}>
          <span className="ico">⚙</span>模型与规则
        </button>
        <div className="sidebar-foot">
          <div className="conn">
            <span className="dot" />
            <span>
              <strong>{cfg.base_url ? "内网网关" : "未配置"}</strong>
              {cfg.base_url ? " · 已配置" : " · 请设置"}
            </span>
          </div>
          <div className="ver">v0.1.0 · Tauri</div>
        </div>
      </aside>

      <div className="main">
        <header className="topbar">
          <div>
            <h1>
              {view === "task" && "新建评审"}
              {view === "history" && "评审历史"}
              {view === "templates" && "模板管理"}
              {view === "settings" && "模型与规则"}
            </h1>
            <div className="sub">
              {view === "task" && "上传混合发布包 → 识别系统 → 分系统评审"}
              {view === "history" && "本机任务与文件级结果"}
              {view === "templates" && "配置评审模板，可自定义与覆盖"}
              {view === "settings" && "内网网关、容量与文件名规则集"}
            </div>
          </div>
          <div className="topbar-actions">
            <span className="chip good">Private Banking</span>
          </div>
        </header>

        {view === "task" && (
          <section className="view">
            <div className="steps">
              {[
                ["1", "选择来源", "ZIP / 目录 / 文本"],
                ["2", "扫描与解析", "识别系统与模板"],
                ["3", "评审配置", "格式 / 粒度 / 分块"],
                ["4", "分析结果", "按系统分组"],
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
                  <span className="hint">文件数 &lt; 800 · 单文件上限可配置</span>
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
                      {uploadPaths.length > 0 && (
                        <div className="preview-box" style={{ maxHeight: 120 }}>
                          {uploadPaths.join("\n")}
                        </div>
                      )}
                    </>
                  )}

                  {source === "dir" && (
                    <>
                      <div className="field">
                        <label>目录路径</label>
                        <div className="inline">
                          <input value={dirPath} onChange={(e) => setDirPath(e.target.value)} placeholder="D:\\release\\2026Q3-mixed" />
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
                    <div className="field">
                      <label>粘贴要评审的内容</label>
                      <textarea placeholder="把配置或文档内容粘贴到这里…（后续版本接入）" />
                    </div>
                  )}

                  <label className="checkline">
                    <input type="checkbox" checked={matchedOnly} onChange={(e) => setMatchedOnly(e.target.checked)} /> 仅保留命中规则的文件
                  </label>

                  <div className="step-foot">
                    <span className="spacer" />
                    <button className="btn btn-primary btn-lg" onClick={doScan} disabled={scanning}>
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
                  <span className="hint">命中规则自动归属系统与模板，可单独覆盖</span>
                  <div className="right">
                    <button className="btn btn-sm" onClick={doScan} disabled={scanning}>
                      重新扫描
                    </button>
                  </div>
                </div>
                <div className="panel-body">
                  <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                    <span className="tag">规则集：{scan.rule_set.name}</span>
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
                          <th style={{ width: 100 }}>归属系统</th>
                          <th style={{ width: 200 }}>评审模板</th>
                          <th style={{ width: 70 }}>分块</th>
                          <th style={{ width: 60 }}></th>
                        </tr>
                      </thead>
                      <tbody>
                        {scan.entries.map((e) => (
                          <tr key={e.path}>
                            <td>
                              <input
                                type="checkbox"
                                checked={e.selected}
                                disabled={!e.matched}
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
                            <td>{e.matched ? <span className={`sys ${systemClass(e.system)}`}>{systemName(e.system)}</span> : <span className="tag skip">未命中</span>}</td>
                            <td>
                              {e.matched ? (
                                <select
                                  className="mini"
                                  value={e.template_id ?? ""}
                                  onChange={(ev) => updateEntry(e.path, { template_id: ev.target.value })}
                                >
                                  {templates.map((t) => (
                                    <option key={t.id} value={t.id}>
                                      {t.name}
                                    </option>
                                  ))}
                                </select>
                              ) : (
                                <span className="tag skip">跳过</span>
                              )}
                            </td>
                            <td>{e.matched ? (e.chunks > 1 ? <span className="verdict conditional">{e.chunks} 块</span> : "1 块") : "—"}</td>
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
                  <h2>评审配置</h2>
                  <span className="hint">模板已在文件清单按系统自动匹配</span>
                </div>
                <div className="panel-body">
                  <div className="field-row">
                    <div className="field">
                      <label>并发数</label>
                      <input type="number" min={1} max={8} value={concurrency} onChange={(e) => setConcurrency(Number(e.target.value) || 1)} />
                    </div>
                    <div className="field">
                      <label>模型</label>
                      <input value={cfg.model || "未配置"} readOnly />
                    </div>
                  </div>
                  <div className="field-row">
                    <div className="field">
                      <label>分块触发阈值（字符）</label>
                      <input type="number" value={threshold} onChange={(e) => setThreshold(Number(e.target.value) || 12000)} />
                    </div>
                    <div className="field">
                      <label>单块大小（字符）</label>
                      <input type="number" value={chunkSize} onChange={(e) => setChunkSize(Number(e.target.value) || 8000)} />
                    </div>
                  </div>
                  <p className="muted" style={{ fontSize: 12 }}>
                    将分析 <strong>{selectedEntries.length}</strong> 个文件；失败单文件自动隔离并计入失败，不影响其他文件。
                  </p>
                  <div className="step-foot">
                    <button className="btn" onClick={() => setStep(2)}>
                      ← 上一步
                    </button>
                    <span className="spacer" />
                    <button className="btn btn-primary btn-lg" onClick={startReview} disabled={running || selectedEntries.length === 0}>
                      {running ? "评审中…" : "开始评审"}
                    </button>
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
                        {running ? "正在评审…" : "评审完成"}
                      </strong>
                      <span className="muted" style={{ marginLeft: "auto", fontFamily: "var(--mono)", fontSize: 12 }}>
                        {doneCount} / {progressList.length || selectedEntries.length}
                      </span>
                    </div>
                    <div className="bar">
                      <i style={{ width: `${progressList.length ? (doneCount / progressList.length) * 100 : 0}%` }} />
                    </div>
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
                    {["apollo", "xxl", "nebula", "other"].map((sys) => {
                      const items = results.filter((r) => (r.system ?? "other") === sys);
                      if (items.length === 0) return null;
                      return (
                        <div className="panel" key={sys} style={{ marginBottom: 15 }}>
                          <div className="group-head">
                            <span className={`sys ${sys}`}>{systemName(sys)}</span>
                            <h3>{items.length} 个文件</h3>
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
                                {p.file} · {p.phase}
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
                            <span className="k">系统：</span>
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
                      <h2>导出（简单版）</h2>
                    </div>
                    <div className="panel-body export-row">
                      <button className="btn" onClick={() => exportCurrent("json")}>
                        导出 JSON
                      </button>
                      <button className="btn" onClick={() => exportCurrent("markdown")}>
                        导出 Markdown
                      </button>
                    </div>
                  </div>
                </div>

                <div className="step-foot">
                  <button className="btn" onClick={() => setStep(3)}>
                    ← 修改配置
                  </button>
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
              <table className="data">
                <thead>
                  <tr>
                    <th>时间</th>
                    <th>来源</th>
                    <th>含系统</th>
                    <th>文件数</th>
                    <th>结论分布</th>
                    <th style={{ width: 150 }}>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {history.length === 0 && (
                    <tr>
                      <td colSpan={6} className="empty">
                        暂无历史。成功评审才会入库。
                      </td>
                    </tr>
                  )}
                  {history.map((h) => (
                    <tr key={h.id}>
                      <td>{formatTime(h.created_at)}</td>
                      <td>
                        {h.source_type === "dir" ? "目录 · " : "文件 · "}
                        {h.source_ref}
                      </td>
                      <td>
                        {h.systems.map((s) => (
                          <span key={s} className={`sys ${s}`} style={{ marginRight: 8 }}>
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
                    <button className="btn btn-sm" onClick={() => setHistoryDetail(null)}>
                      关闭
                    </button>
                  </div>
                </div>
                <table className="data">
                  <thead>
                    <tr>
                      <th>文件</th>
                      <th style={{ width: 90 }}>系统</th>
                      <th style={{ width: 110 }}>结论</th>
                    </tr>
                  </thead>
                  <tbody>
                    {historyDetail.files.map((f) => (
                      <tr key={f.file}>
                        <td>{f.file}</td>
                        <td>
                          <span className={`sys ${systemClass(f.system)}`}>{systemName(f.system)}</span>
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

        {view === "templates" && (
          <section className="view">
            <div className="two-col">
              <div className="panel">
                <div className="panel-head">
                  <h2>评审模板</h2>
                </div>
                <div>
                  {templates.map((t) => (
                    <div
                      key={t.id}
                      className={`tpl-item ${editingTemplate?.id === t.id ? "is-active" : ""}`}
                      onClick={() => setEditingTemplate(t)}
                    >
                      <div>
                        <strong>{t.name}</strong>
                        <small>{t.content}</small>
                      </div>
                      <span className="tag">{t.builtin ? "内置" : "自定义"}</span>
                    </div>
                  ))}
                </div>
              </div>
              <div className="panel">
                <div className="panel-head">
                  <h2>编辑模板</h2>
                </div>
                <div className="panel-body">
                  {editingTemplate ? (
                    <>
                      <div className="field">
                        <label>模板名称</label>
                        <input
                          value={editingTemplate.name}
                          onChange={(e) => setEditingTemplate({ ...editingTemplate, name: e.target.value })}
                        />
                      </div>
                      <div className="field">
                        <label>Prompt 内容</label>
                        <textarea
                          value={editingTemplate.content}
                          onChange={(e) => setEditingTemplate({ ...editingTemplate, content: e.target.value })}
                        />
                      </div>
                      <p className="muted" style={{ fontSize: 12 }}>
                        模板编辑暂为本地会话内存；持久化在后继版本接入。
                      </p>
                    </>
                  ) : (
                    <div className="empty">选择左侧模板进行编辑</div>
                  )}
                </div>
              </div>
            </div>
          </section>
        )}

        {view === "settings" && (
          <section className="view">
            <div className="two-col">
              <div className="panel">
                <div className="panel-head">
                  <h2>模型与连接</h2>
                  <span className="hint">模型由使用者自行配置</span>
                </div>
                <div className="panel-body">
                  <div className="field">
                    <label>Base URL</label>
                    <input
                      value={cfg.base_url}
                      placeholder="http://llm-gateway.intra.corp/v1"
                      onChange={(e) => setCfg({ ...cfg, base_url: e.target.value })}
                    />
                  </div>
                  <div className="field-row">
                    <div className="field">
                      <label>模型标识</label>
                      <input value={cfg.model} placeholder="例如 deepseek-v4-flash" onChange={(e) => setCfg({ ...cfg, model: e.target.value })} />
                    </div>
                    <div className="field">
                      <label>并发数</label>
                      <input type="number" min={1} max={8} value={concurrency} onChange={(e) => setConcurrency(Number(e.target.value) || 1)} />
                    </div>
                  </div>
                  <div className="field">
                    <label>API Key</label>
                    <input type="password" value={cfg.api_key} placeholder="sk-…" onChange={(e) => setCfg({ ...cfg, api_key: e.target.value })} />
                  </div>
                  <div className="field-row">
                    <div className="field">
                      <label>读取超时（秒）</label>
                      <input type="number" value={cfg.timeout_secs} onChange={(e) => setCfg({ ...cfg, timeout_secs: Number(e.target.value) || 180 })} />
                    </div>
                    <div className="field">
                      <label>单文件上限（MB）</label>
                      <input value={1228} readOnly />
                    </div>
                  </div>
                  <label className="checkline">
                    <input type="checkbox" checked={cfg.allow_http} onChange={(e) => setCfg({ ...cfg, allow_http: e.target.checked })} /> 允许内网 HTTP（无证书网关）
                  </label>
                  <div style={{ display: "flex", gap: 9, marginTop: 14 }}>
                    <button
                      className="btn"
                      onClick={async () => {
                        try {
                          const reply = await api.testConnection(cfg);
                          toast(`连接正常：${reply}`);
                        } catch (e) {
                          toast(String(e));
                        }
                      }}
                    >
                      测试连接
                    </button>
                    <button className="btn btn-primary" onClick={() => { saveSettings(cfg); toast("设置已保存"); }}>
                      保存设置
                    </button>
                  </div>
                  <p className="muted" style={{ fontSize: 11.5, marginTop: 10 }}>
                    注意：API Key 目前保存在浏览器本地存储，后续版本接入 Windows DPAPI 加密。
                  </p>
                </div>
              </div>

              <div className="panel">
                <div className="panel-head">
                  <h2>文件名规则集</h2>
                  <span className="hint">预置一套，可增删改</span>
                </div>
                <div className="panel-body" style={{ paddingTop: 16 }}>
                  {ruleSet ? (
                    <>
                      <div className="field">
                        <label>规则集名称</label>
                        <input value={ruleSet.name} onChange={(e) => setRuleSet({ ...ruleSet, name: e.target.value })} />
                      </div>
                      <table className="data">
                        <thead>
                          <tr>
                            <th>匹配模式（glob）</th>
                            <th style={{ width: 110 }}>归属系统</th>
                            <th style={{ width: 180 }}>评审模板</th>
                            <th style={{ width: 44 }}></th>
                          </tr>
                        </thead>
                        <tbody>
                          {ruleSet.rules.map((r) => (
                            <tr key={r.id}>
                              <td>{r.pattern}</td>
                              <td>
                                <span className={`sys ${systemClass(r.system)}`}>{systemName(r.system)}</span>
                              </td>
                              <td>{templates.find((t) => t.id === r.template_id)?.name ?? r.template_id}</td>
                              <td>
                                <button
                                  className="link danger"
                                  onClick={() => setRuleSet({ ...ruleSet, rules: ruleSet.rules.filter((x) => x.id !== r.id) })}
                                >
                                  删
                                </button>
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                      <p className="muted" style={{ fontSize: 11.5, marginTop: 10 }}>
                        规则集编辑暂为本地会话内存；持久化在后继版本接入。
                      </p>
                    </>
                  ) : (
                    <div className="empty">正在加载…</div>
                  )}
                </div>
              </div>
            </div>
          </section>
        )}
      </div>

      {notice && <div className="toast is-visible">{notice}</div>}
    </div>
  );
}
