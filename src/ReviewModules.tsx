import { useState } from "react";
import type { AppConfig, Defaults, OutputFormat, Rule, Template } from "./types";

export default function ReviewModules({ config, defaults, activeRuleSetId, onRuleSetChange, onChange, onSave, disabled, dirty }: {
  config: AppConfig; defaults: Defaults | null; activeRuleSetId: string;
  onRuleSetChange: (id: string) => void; onChange: (patch: Partial<AppConfig>) => void;
  onSave: () => Promise<void>; disabled: boolean; dirty: boolean;
}) {
  const [selectedId, setSelectedId] = useState(config.systems[0]?.id ?? "");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleteId, setDeleteId] = useState<string | null>(null);
  const selected = config.systems.find((m) => m.id === selectedId) ?? config.systems[0];
  const template = config.templates.find((t) => t.id === selected?.default_template_id);
  const scheme = config.rule_sets.find((r) => r.id === activeRuleSetId) ?? config.rule_sets[0];
  const moduleRules = scheme?.rules.filter((r) => r.system === selected?.id) ?? [];
  const change = (patch: Partial<AppConfig>) => { setMessage(""); setError(""); onChange(patch); };
  const editTemplate = (patch: Partial<Template>) => {
    if (template) change({ templates: config.templates.map((t) => t.id === template.id ? { ...t, ...patch, overridden: true } : t) });
  };
  const editRule = (id: string, patch: Partial<Rule>) => change({ rule_sets: config.rule_sets.map((rs) => rs.id === scheme?.id ? { ...rs, rules: rs.rules.map((r) => r.id === id ? { ...r, ...patch } : r) } : rs) });
  const createModule = () => {
    const id = `module-${crypto.randomUUID()}`;
    const tplId = `tpl-${id}`;
    change({ systems: [...config.systems, { id, name: "新评审模块", color: "#3a5a78", default_template_id: tplId }], templates: [...config.templates, {
      id: tplId, name: "新评审模块评审规范", content: "核对资料完整性、配置一致性、风险与回滚方案。未提供的资料标为待确认，不得自行假定。",
      input_instructions: "输入为文件解析后的正文或表格，保留文件名、行号和原文定位。", output_instructions: "结论应说明依据、风险及可执行的整改建议，并引用原文。",
      output_format: "standard", builtin: false, overridden: false,
    }] });
    setSelectedId(id);
  };
  const addRule = () => {
    if (!selected || !template) return;
    const rule: Rule = { id: `r-${crypto.randomUUID()}`, pattern: "", match_type: "keyword", system: selected.id, template_id: template.id, enabled: true };
    if (!scheme) {
      const id = `rs-${crypto.randomUUID()}`;
      change({ rule_sets: [{ id, name: "模块文件匹配", builtin: false, rules: [rule] }] });
      onRuleSetChange(id);
    } else {
      change({ rule_sets: config.rule_sets.map((rs) => rs.id === scheme.id ? { ...rs, rules: [rule, ...rs.rules] } : rs) });
    }
  };
  const moveRule = (index: number, offset: number) => {
    if (!scheme) return;
    const rules = [...scheme.rules];
    [rules[index], rules[index + offset]] = [rules[index + offset], rules[index]];
    change({ rule_sets: config.rule_sets.map((rs) => rs.id === scheme.id ? { ...rs, rules } : rs) });
  };
  const removeModule = () => {
    const removed = config.systems.find((m) => m.id === deleteId);
    if (!removed) return;
    const systems = config.systems.filter((m) => m.id !== removed.id);
    change({ systems, templates: config.templates.filter((t) => t.id !== removed.default_template_id || systems.some((m) => m.default_template_id === t.id)),
      rule_sets: config.rule_sets.map((rs) => ({ ...rs, rules: rs.rules.filter((r) => r.system !== removed.id) })) });
    setDeleteId(null); setSelectedId(systems[0]?.id ?? "");
  };
  const save = async () => {
    setError(""); setMessage(""); setSaving(true);
    try { await onSave(); setMessage("评审模块已保存，后续扫描和评审使用此规范。"); }
    catch (e) { setError(String(e)); }
    finally { setSaving(false); }
  };
  const formatHelp: Record<OutputFormat, string> = {
    standard: "固定结构：通过 / 有条件通过 / 阻断；依据、风险、整改项及原文引用。",
    checklist: "先给结论，再用检查项、结论、依据组成的表格逐项列出，最后列出风险与整改项。",
    brief: "先给结论，再用不超过 5 条要点说明依据、风险及整改项。",
    json: "固定字段：verdict、basis、risks、remediations、citations。程序据此解析和导出。",
  };
  return <section className="view">
    <div className="module-intro"><div><strong>按模块配置评审</strong><p>依次设置文件匹配、评审要求和输出结果，保存后即可使用。</p></div>
      <span className="tag">{dirty ? "有未保存的配置" : "配置已保存"}</span></div>
    {disabled && <p role="status">扫描或评审运行中，完成后可修改模块。</p>}
    <fieldset className="module-fieldset" disabled={disabled || saving}>
      <div className="module-layout">
        <aside className="panel module-list">
          <div className="panel-head"><h2>评审模块</h2><button className="btn btn-sm" onClick={createModule}>新建模块</button></div>
          {config.systems.map((m) => <button key={m.id} className={`module-item ${selected?.id === m.id ? "is-active" : ""}`} onClick={() => { setSelectedId(m.id); setMessage(""); }} aria-pressed={selected?.id === m.id}>
            <strong><i style={{ background: m.color }} />{m.name}</strong><small>{scheme?.rules.filter((r) => r.system === m.id && r.enabled).length ?? 0} 条启用的匹配条件</small>
          </button>)}
          {config.systems.length === 0 && <div className="empty">新建模块后配置文件匹配与评审规范。</div>}
          {config.rule_sets.length > 1 && <div className="panel-body"><label htmlFor="module-scheme">文件匹配方案</label><select id="module-scheme" value={scheme?.id} onChange={(e) => onRuleSetChange(e.target.value)}>
            {config.rule_sets.map((rs) => <option key={rs.id} value={rs.id}>{rs.name}</option>)}</select><small>保留旧版的多个匹配方案。模块规范在所有方案中共用。</small></div>}
        </aside>
        <div className="panel module-editor">
          <div className="panel-head"><h2>{selected?.name ?? "配置评审模块"}</h2><div className="right">
            {selected && <button className="btn btn-sm" onClick={() => setDeleteId(selected.id)}>删除模块</button>}
            <button className="btn btn-primary" onClick={save}>{saving ? "保存中…" : "保存模块配置"}</button>
          </div></div>
          <div className="panel-body">
            {error && <p className="error-text" role="alert">{error}</p>}{message && <p className="module-success" role="status">{message}</p>}
            {selected && template ? <>
              <div className="field-row"><div className="field"><label htmlFor="module-name">模块名称</label><input id="module-name" value={selected.name} onChange={(e) => change({ systems: config.systems.map((m) => m.id === selected.id ? { ...m, name: e.target.value } : m) })} /></div>
                <div className="field module-color"><label htmlFor="module-color">标识颜色</label><input id="module-color" type="color" value={selected.color} onChange={(e) => change({ systems: config.systems.map((m) => m.id === selected.id ? { ...m, color: e.target.value } : m) })} /></div></div>
              <section className="module-section"><div className="inline"><h3><span className="module-step">1</span> 文件匹配</h3><span className="spacer" /><button className="btn btn-sm" onClick={addRule}>添加匹配条件</button></div>
                <p className="muted">任一条件命中就归入本模块。不区分大小写，关键字只匹配文件名；例如“阿波罗”可匹配阿波罗.txt、阿波罗1.docx。</p>
                {moduleRules.map((r, index) => <div className="module-rule" key={r.id}>
                  <input type="checkbox" aria-label={`启用匹配 ${index + 1}`} checked={r.enabled} onChange={(e) => editRule(r.id, { enabled: e.target.checked })} />
                  <select aria-label={`匹配方式 ${index + 1}`} value={r.match_type ?? "glob"} onChange={(e) => editRule(r.id, { match_type: e.target.value as Rule["match_type"] })}>
                    <option value="keyword">文件名包含</option><option value="filename">文件名完全等于</option><option value="extension">文件后缀</option><option value="glob">高级 glob</option></select>
                  <input aria-label={`匹配条件 ${index + 1}`} value={r.pattern} placeholder={r.match_type === "extension" ? "txt" : "例如：阿波罗"} onChange={(e) => editRule(r.id, { pattern: e.target.value })} />
                  <button className="link danger" aria-label={`删除匹配 ${index + 1}`} onClick={() => change({ rule_sets: config.rule_sets.map((rs) => rs.id === scheme?.id ? { ...rs, rules: rs.rules.filter((x) => x.id !== r.id) } : rs) })}>删除</button>
                </div>)}
                {moduleRules.length === 0 && <p className="muted">尚未配置匹配条件，仅可用于粘贴文本。添加条件后即可自动识别文件。</p>}
                <small>匹配不代表能够解析。支持 TXT、JSON、Excel、DOC / DOCX，PDF 暂不支持。</small>
              </section>
              <section className="module-section"><h3><span className="module-step">2</span> 评审要求</h3><p className="muted">写清楚要检查什么、什么情况通过或阻断。文件正文会自动带入。</p>
                <div className="field"><label htmlFor="module-review">评审要求</label><textarea id="module-review" className="review-prompt" value={template.content} onChange={(e) => editTemplate({ content: e.target.value })} /></div>
                <details className="module-advanced" key={`${selected.id}-input`}><summary>输入说明（可选）</summary>
                  <div className="field"><label htmlFor="module-input">输入说明</label><textarea id="module-input" value={template.input_instructions} placeholder="说明输入是什么资料，以及如何理解表格、字段和缺失内容。" onChange={(e) => editTemplate({ input_instructions: e.target.value })} /></div>
                </details>
              </section>
              <section className="module-section"><h3><span className="module-step">3</span> 输出结果</h3>
                <div className="field"><label htmlFor="module-format">输出格式</label><select id="module-format" value={template.output_format} onChange={(e) => editTemplate({ output_format: e.target.value as OutputFormat })}>
                  <option value="standard">标准结论</option><option value="checklist">检查表</option><option value="brief">简洁摘要</option><option value="json">JSON</option></select><small>{formatHelp[template.output_format]}</small></div>
                <details className="module-advanced" key={`${selected.id}-output`}><summary>输出补充要求（可选）</summary>
                  <div className="field"><label htmlFor="module-output">输出补充要求</label><textarea id="module-output" value={template.output_instructions} placeholder="补充报告措辞、引用粒度和整改建议要求，遵循上方输出结构。" onChange={(e) => editTemplate({ output_instructions: e.target.value })} /></div>
                </details>
                {defaults?.templates.some((t) => t.id === template.id) && <button className="btn btn-sm" onClick={() => { const original = defaults.templates.find((t) => t.id === template.id)!; editTemplate({ content: original.content, input_instructions: original.input_instructions, output_instructions: original.output_instructions, output_format: original.output_format }); }}>恢复内置规范</button>}
              </section>
            </> : <div className="empty">选择或新建一个模块。</div>}
          </div>
        </div>
      </div>
      {scheme && scheme.rules.length > 0 && <details className="panel module-priority"><summary>匹配优先级 · 多个模块命中时采用第一条启用条件</summary><div className="panel-body">
        <p className="muted">新条件默认置顶。可调整全部模块条件的先后顺序，修改后保存模块配置。</p>
        <ol>{scheme.rules.map((r, index) => <li key={r.id}><span>{config.systems.find((m) => m.id === r.system)?.name} · <code>{r.pattern || "待填写"}</code>{!r.enabled && "（停用）"}</span>
          <button className="btn btn-sm" aria-label={`上移优先级 ${index + 1}`} disabled={index === 0} onClick={() => moveRule(index, -1)}>↑</button><button className="btn btn-sm" aria-label={`下移优先级 ${index + 1}`} disabled={index === scheme.rules.length - 1} onClick={() => moveRule(index, 1)}>↓</button></li>)}</ol>
      </div></details>}
    </fieldset>
    {deleteId && <div className="modal-mask"><div className="modal" role="dialog" aria-modal="true" aria-labelledby="delete-module-title"><div className="modal-head"><h2 id="delete-module-title">删除评审模块</h2></div><div className="modal-body">删除“{config.systems.find((m) => m.id === deleteId)?.name}”及其匹配条件和规范？已有历史结果会保留，删除后需保存配置。</div><div className="modal-foot"><button className="btn" onClick={() => setDeleteId(null)}>取消</button><button className="btn" onClick={removeModule}>确认删除模块</button></div></div></div>}
  </section>;
}
