import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import type { RuleSet, ScheduleConfig, ScheduledJob } from "./types";

const time = (value: number | null | undefined) => value ? new Date(value).toLocaleString("zh-CN", { hour12: false }) : "—";
const states: Record<string, string> = { running: "正在执行", succeeded: "评审完成", failed: "执行失败", cancelled: "已取消", interrupted: "运行中断", unchanged: "无变化，已跳过", no_matches: "没有匹配文件" };
const phases: Record<string, string> = { git: "1. 拉取 Git 最新版本", discover: "2. 统计文件", extract: "2. 解压 ZIP", parse: "2. 解析资料", done: "2. 扫描完成", snapshot: "3. 保存本轮输入", compare: "3. 检查内容变化", review: "4. 自动评审" };

export default function Schedules({ jobs, error, ruleSets, defaultRuleSet, busy, beforeSave, refresh, openHistory }: {
  jobs: ScheduledJob[]; error: string; ruleSets: RuleSet[]; defaultRuleSet: string; busy: boolean;
  beforeSave: () => Promise<void>; refresh: () => Promise<void>; openHistory: (id: string) => void;
}) {
  const [draft, setDraft] = useState<ScheduleConfig | null>(null);
  const [working, setWorking] = useState(false);
  const [actionError, setActionError] = useState("");
  const [notice, setNotice] = useState("");
  const [deleteId, setDeleteId] = useState("");
  const active = jobs.some(j => j.last_run?.state === "running");
  const act = async (action: () => Promise<void>, success = "") => {
    setWorking(true); setActionError(""); setNotice("");
    try { await action(); await refresh(); setNotice(success); }
    catch (e) { setActionError(String(e)); }
    finally { setWorking(false); }
  };
  const edit = (patch: Partial<ScheduleConfig>) => setDraft(d => d ? { ...d, ...patch } : d);
  const newJob = () => {
    setActionError(""); setNotice("");
    setDraft({ id: crypto.randomUUID(), name: "定期配置评审", directory: "", interval_minutes: 60, enabled: true, git_pull: true, only_changed: true, rule_set_id: defaultRuleSet });
  };
  return <section className="view schedule-view">
    <div className="panel">
      <div className="panel-head"><h2>定时自动评审</h2><button className="btn btn-primary" disabled={working} onClick={newJob}>新建定时任务</button></div>
      <div className="panel-body">
        <p className="schedule-flow">拉取 Git → 扫描最新资料 → 按模块逐文件评审 → 保存历史</p>
        <p className="muted">关闭窗口后在托盘继续运行；从托盘选择「退出」后停止。间隔从上一轮结束后计算，休眠或退出期间不会执行。</p>
        <p className="muted">沿用已保存的评审模块和模型连接。多个任务依次执行；手动扫描或评审期间，定时任务会等待。</p>
      </div>
    </div>
    {(error || actionError) && <div className="schedule-message error-text" role="alert">{actionError || error}</div>}
    {notice && <div className="schedule-message" role="status">{notice}</div>}
    {draft && <form className="panel schedule-editor" onSubmit={e => {
      e.preventDefault();
      void act(async () => { await beforeSave(); await api.saveSchedule(draft); setDraft(null); }, "任务已保存；到时自动执行，也可以点击「立即执行」。");
    }}>
      <div className="panel-head"><h2>{jobs.some(j => j.config.id === draft.id) ? "编辑定时任务" : "新建定时任务"}</h2></div>
      <fieldset disabled={working} className="panel-body">
        <div className="field"><label htmlFor="schedule-name">任务名称</label><input id="schedule-name" required maxLength={80} value={draft.name} onChange={e => edit({ name: e.target.value })} /></div>
        <div className="field"><label htmlFor="schedule-directory">资料目录</label><div className="inline"><input id="schedule-directory" required value={draft.directory} placeholder="选择已克隆的本地仓库或其子目录" onChange={e => edit({ directory: e.target.value })} /><button type="button" className="btn" onClick={() => void act(async () => { const path = await open({ directory: true }); if (typeof path === "string") edit({ directory: path }); })}>浏览…</button></div><small>递归扫描子目录，自动忽略 .git；只评审命中文件匹配规则的资料。</small></div>
        <div className="field-row"><div className="field"><label htmlFor="schedule-interval">执行间隔（分钟）</label><input id="schedule-interval" type="number" required min={1} max={525600} value={draft.interval_minutes || ""} onChange={e => edit({ interval_minutes: Number(e.target.value) })} /></div>
        {ruleSets.length > 1 && <div className="field"><label htmlFor="schedule-rules">文件匹配方案</label><select id="schedule-rules" value={draft.rule_set_id} onChange={e => edit({ rule_set_id: e.target.value })}>{ruleSets.map(r => <option key={r.id} value={r.id}>{r.name}</option>)}</select></div>}</div>
        <label className="checkline"><input type="checkbox" checked={draft.git_pull} onChange={e => edit({ git_pull: e.target.checked })} />执行前 Git 拉取</label>
        {draft.git_pull && <p className="muted schedule-help">需要电脑已安装 Git，仓库已设置上游且能免交互登录。有本地修改或分支分叉时，本轮停止并显示原因。</p>}
        <label className="checkline"><input type="checkbox" checked={draft.only_changed} onChange={e => edit({ only_changed: e.target.checked })} />仅内容或评审配置变化时评审</label>
        <label className="checkline"><input type="checkbox" checked={draft.enabled} onChange={e => edit({ enabled: e.target.checked })} />启用定时执行</label>
        <div className="step-foot"><button type="button" className="btn" onClick={() => setDraft(null)}>取消编辑</button><span className="spacer" /><button className="btn btn-primary" type="submit">{working ? "正在保存…" : "保存任务"}</button></div>
      </fieldset>
    </form>}
    {!jobs.length && !draft && !error && <div className="empty">还没有定时任务。新建后选择资料目录和执行间隔即可。</div>}
    {jobs.map(job => {
      const c = job.config, r = job.last_run, running = r?.state === "running";
      const count = r?.total != null ? `${r.processed} / ${r.total}` : `已发现 ${r?.discovered ?? 0} 个文件`;
      return <article className="panel schedule-card" data-testid="schedule-card" key={c.id} aria-label={c.name}>
        <div className="panel-head"><h2>{c.name}</h2><span className={`verdict ${running ? "conditional" : c.enabled ? "pass" : ""}`}>{running ? "正在执行" : c.enabled ? "定时已启用" : "定时已暂停"}</span></div>
        <div className="panel-body">
          <p className="schedule-path" title={c.directory}>{c.directory}</p>
          <div className="schedule-meta"><span>每 {c.interval_minutes} 分钟</span><span>{c.git_pull ? "先拉取 Git" : "直接扫描"}</span><span>{c.only_changed ? "变化后评审" : "每轮评审"}</span></div>
          <p className="muted">下次执行：{running ? "本轮结束后计算" : c.enabled ? time(job.next_run_at) : "已暂停"}</p>
          {r && <div className="schedule-run" aria-live="polite">
            <div className="inline"><strong>{running ? phases[r.phase] || "正在处理" : states[r.state] || r.state}</strong><span className="spacer" />{running && <strong className="progress-count">{count}</strong>}</div>
            {running && <><progress aria-label={`${c.name}进度`} max={r.total || 1} value={r.total == null ? undefined : r.total === 0 ? 1 : r.processed} /><div className="scan-current" title={r.current}>{r.current || r.message || "准备中…"}</div>{r.phase === "review" && <small className="muted">按完成的文件计数，包含失败；正在等待模型的文件完成后更新。</small>}</>}
            {!running && <p className={r.state === "failed" ? "error-text" : ""}>{r.message}</p>}
            <small className="muted">开始 {time(r.started_at)}{r.finished_at ? ` · 结束 ${time(r.finished_at)}` : ""}</small>
            {r.after_commit && <p className="muted">Git 版本 {r.before_commit.slice(0, 8)} → {r.after_commit.slice(0, 8)}</p>}
          </div>}
          <div className="schedule-actions">
            {running ? <button className="btn" disabled={working} onClick={() => void act(() => api.cancelSchedule(c.id), "已请求取消；正在结束当前步骤。")} >取消本次</button> : <button className="btn btn-primary" disabled={working || active || busy} onClick={() => void act(async () => { await beforeSave(); await api.runScheduleNow(c.id); })}>立即执行</button>}
            <button className="btn" disabled={working || running} onClick={() => void act(() => api.saveSchedule({ ...c, enabled: !c.enabled }), c.enabled ? "已暂停定时执行。" : "已启用定时执行。")}>{c.enabled ? "暂停" : "启用"}</button>
            <button className="btn" disabled={working || running} onClick={() => { setDraft({ ...c }); setActionError(""); }}>编辑</button>
            {r?.history_id && <button className="btn" onClick={() => openHistory(r.history_id!)}>查看本次报告</button>}
            <span className="spacer" />
            {deleteId === c.id ? <><span>删除此任务？历史报告会保留。</span><button className="btn danger" disabled={working || running} onClick={() => void act(async () => { await api.deleteSchedule(c.id); setDeleteId(""); if (draft?.id === c.id) setDraft(null); })}>确认删除</button><button className="btn" onClick={() => setDeleteId("")}>保留</button></> : <button className="link" disabled={working || running} onClick={() => setDeleteId(c.id)}>删除</button>}
          </div>
        </div>
      </article>;
    })}
  </section>;
}
