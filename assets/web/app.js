// Passo · painel. Live: polls /api/state. Report: reads the snapshot embedded in the page.
(function () {
  "use strict";
  const boot = window.PASSO || {};
  const live = boot.mode === "live";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const STAGES = ["refinar", "planejar", "construir", "verificar", "entregar"];
  const LABEL = { refinar: "Refinar", planejar: "Planejar", construir: "Construir", verificar: "Verificar", entregar: "Entregar" };
  const SIZE = { pp: "PP · ajuste", p: "P · pequeno", m: "M · médio", g: "G · grande" };
  const KIND = { resposta: "resposta", recusa: "recusa", mudanca: "mudança no pedido", tamanho: "tamanho", plano: "nova versão do plano", dica: "dica" };
  let state = null;
  let selected = "";
  let gateKey = "";
  let filter = "tudo";

  const ms = (n) => {
    const s = Math.round((n || 0) / 1000);
    if (s < 60) return s + "s";
    if (s < 3600) return Math.floor(s / 60) + "min " + (s % 60) + "s";
    return Math.floor(s / 3600) + "h " + Math.floor((s % 3600) / 60) + "min";
  };
  const num = (n) => (n < 1000 ? String(n) : n < 1e6 ? (n / 1000).toFixed(1) + "k" : (n / 1e6).toFixed(2) + "M");
  const billable = (t) => (t ? t.input + t.output + t.cache_write : 0);
  const time = (t) => new Date(t).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit", second: "2-digit" });

  // ---------------------------------------------------------------------------------------------- rail
  function rail(s) {
    const run = s.run;
    const m = s.metrics;
    $("rail").innerHTML = s.stages
      .map((st, i) => {
        let cls = "todo";
        if (run) {
          const cur = STAGES.indexOf(run.stage);
          if (run.status === "done" || i < cur) cls = "done";
          else if (i === cur && run.status === "active") cls = "current";
        }
        const who = st.id === "entregar" ? "o Passo · você aceita" : st.mode === "auto" ? "automático" : (st.agent || "agente") + " · " + (st.tier || "");
        const sm = m && m.stages[i];
        const nums = sm && sm.visits ? ms(sm.ms) + (sm.tokens.calls ? " · " + num(billable(sm.tokens)) + " tok" : "") : "";
        const auto = st.mode === "auto" && st.id !== "entregar" ? '<span class="tag">auto</span>' : "";
        return `<div class="stage ${cls}"><div class="n">0${i + 1}</div><div class="name">${esc(st.label)} ${auto}</div><div class="who">${esc(who)}</div><div class="num">${nums || "&nbsp;"}</div></div>`;
      })
      .join("");
  }

  // ---------------------------------------------------------------------------------------------- now
  function now(s) {
    const el = $("now");
    const run = s.run;
    if (!run) {
      el.className = "card now";
      el.innerHTML = `<div class="empty">Nenhum run ainda.<br>No agente, peça algo “com o passo”, ou rode <code>passo start "pedido"</code>.</div>`;
      gateKey = "";
      return;
    }
    if (run.status !== "active") {
      el.className = "card now done";
      el.innerHTML = `<div class="turn">${run.status === "done" ? "Concluído" : "Interrompido"}</div><h3>${esc(run.request)}</h3><p class="muted small">${run.proof ? "Prova " + esc(proof(run.proof)) + " · " : ""}${esc(run.id)}</p>`;
      gateKey = "";
      return;
    }
    if (run.gate) {
      const key = run.id + run.gate.kind + run.gate.at + run.plan_version;
      if (key === gateKey) return; // keep the form while the dev is choosing
      gateKey = key;
      el.className = "card now dev";
      el.innerHTML = gate(run);
      wire(run);
      return;
    }
    gateKey = "";
    el.className = "card now";
    const step = run.steps.find((x) => x.status === "doing");
    const lastTool = [...s.events].reverse().find((e) => e.kind === "tool" || e.kind === "lock.denied");
    const what = run.stage === "construir" && step
      ? `<h3>${esc(step.id)} · ${esc(step.title)}</h3>${step.test ? `<p class="muted small">Teste: ${esc(step.test)}</p>` : ""}${step.attempts ? `<p class="small" style="color:var(--warn)">Tentativa ${step.attempts + 1}</p>` : ""}`
      : `<h3>${esc(LABEL[run.stage])}</h3>`;
    const doing = lastTool ? `<p class="faint small mono">${esc(lastTool.text)}</p>` : "";
    const fb = run.feedback ? `<details class="check"><summary><span class="small" style="color:var(--warn)">O que o Passo devolveu ao agente</span></summary><pre>${esc(run.feedback)}</pre></details>` : "";
    el.innerHTML = `<div class="turn">Vez do agente</div>${what}${doing}${fb}`;
  }

  function proof(p) {
    return { forte: "forte", media: "média", fraca: "fraca", nenhuma: "nenhuma" }[p] || p;
  }

  function questionsHtml(run, ids, single) {
    const qs = run.questions.filter((q) => ids.includes(q.id));
    return qs
      .map((q, i) => {
        const opts = q.options
          .map((o) => `<label class="opt"><input type="radio" name="q${i}" value="${esc(o.id)}" ${o.id === q.recommended ? "checked" : ""}><span><b>${esc(o.id)}</b> ${esc(o.text)}${o.id === q.recommended ? '<span class="rec">recomendada</span>' : ""}${o.why ? `<br><span class="faint small">${esc(o.why)}</span>` : ""}</span></label>`)
          .join("");
        return `<div class="q" data-i="${i}" data-rec="${esc(q.recommended)}"><div class="ask">${single ? "" : i + 1 + ". "}${esc(q.ask)}</div>${q.why ? `<p class="faint small">${esc(q.why)}</p>` : ""}${opts}<label class="opt"><input type="radio" name="q${i}" value="__own"><span class="muted">Minhas palavras</span></label><input type="text" class="own" data-i="${i}" placeholder="Sua resposta" style="display:none;margin-top:4px"></div>`;
      })
      .join("");
  }

  function gate(run) {
    const g = run.gate;
    const head = `<div class="turn">Sua vez · ${esc({ aprovar: "aprovar o plano", aceitar: "aceitar a entrega", pergunta: "responder", travado: "destravar", limite: "limite do tamanho" }[g.kind])}</div>`;
    const reject = (label) => `<div class="reject" id="reject"><textarea id="reason" placeholder="${esc(label)}"></textarea><div class="actions"><button id="send-no">Enviar</button></div></div>`;
    if (g.kind === "aprovar") {
      const b = run.brief || {};
      const steps = run.steps.filter((s) => s.status !== "done");
      return `${head}<h3>${esc(run.plan ? run.plan.title : "Plano")} <span class="faint small">v${run.plan_version} · ${esc(SIZE[run.size])}</span></h3>
        <p class="muted">${esc(b.understanding || "")}</p>
        <div class="cols"><div><h2>Critérios</h2><ul class="list">${run.criteria.map((c) => `<li><span class="id">${esc(c.id)}</span><span class="body">${esc(c.text)}</span></li>`).join("")}</ul></div>
        <div><h2>Passos</h2><ul class="list">${steps.map((s) => `<li><span class="id">${esc(s.id)}</span><span class="body">${esc(s.title)}${s.test ? `<div class="meta">teste: ${esc(s.test)}</div>` : ""}${s.test_command ? `<div class="meta mono">o Passo roda: ${esc(s.test_command)}</div>` : ""}</span></li>`).join("")}</ul></div></div>
        ${run.plan && run.plan.notes ? `<p class="small muted" style="margin-top:12px">${esc(run.plan.notes)}</p>` : ""}
        ${questionsHtml(run, g.questions, false)}
        ${steps.some((s) => s.test_command) ? '<p class="small" style="margin-top:12px;color:var(--accent)">Aprovar também autoriza o Passo a rodar os comandos de teste listados nos passos.</p>' : ""}
        <div class="actions"><button class="primary" id="ok">Aprovar</button><button id="no">Recusar…</button><span class="cmd" id="cmd">passo ok</span></div>
        ${reject("O que mudar no pedido ou no plano")}<div class="err" id="err"></div>`;
    }
    if (g.kind === "aceitar") {
      return `${head}<h3>${esc(run.request)}</h3><p class="muted small">Prova ${esc(proof(run.proof))} · ${run.steps.filter((s) => s.status === "done").length} passo(s) · ${run.rounds} volta(s)</p>
        <div class="actions"><button class="primary" id="ok">Aceitar a entrega</button><button id="no">Pedir ajuste…</button><span class="cmd">passo ok</span></div>
        ${reject("O que ajustar")}<div class="err" id="err"></div>`;
    }
    if (g.kind === "pergunta") {
      return `${head}${questionsHtml(run, g.questions, true)}<div class="actions"><button class="primary" id="ok">Responder</button><span class="cmd" id="cmd">passo ok</span></div><div class="err" id="err"></div>`;
    }
    if (g.kind === "travado") {
      return `${head}<h3>${esc(g.title)}</h3><pre class="mono small" style="white-space:pre-wrap">${esc(g.detail)}</pre><input type="text" id="hint" placeholder="Uma dica para o agente (opcional)"><div class="actions"><button class="primary" id="ok">Tentar de novo</button><button id="stop">Encerrar o run</button><span class="cmd">passo ok "dica"</span></div><div class="err" id="err"></div>`;
    }
    return `${head}<h3>${esc(g.title)}</h3><p class="muted">${esc(g.detail)}</p><div class="actions"><button class="primary" id="ok">Seguir assim</button>${["p", "m", "g"].filter((x) => x !== run.size).map((x) => `<button data-size="${x}">Subir para ${x.toUpperCase()}</button>`).join("")}<span class="cmd">passo ok · passo size m</span></div><div class="err" id="err"></div>`;
  }

  function answers(run) {
    const out = [];
    document.querySelectorAll("#now .q").forEach((q) => {
      const i = Number(q.dataset.i);
      const picked = q.querySelector("input[type=radio]:checked");
      if (!picked) return;
      if (picked.value === "__own") {
        const own = q.querySelector(".own").value.trim();
        if (own) out.push(`${i + 1}=${own}`);
      } else if (picked.value !== q.dataset.rec) {
        out.push(`${i + 1}=${picked.value}`);
      }
    });
    return out;
  }

  function wire(run) {
    const el = $("now");
    const cmd = $("cmd");
    const refresh = () => {
      el.querySelectorAll(".q").forEach((q) => {
        const own = q.querySelector(".own");
        own.style.display = q.querySelector("input[value=__own]").checked ? "block" : "none";
      });
      if (cmd) {
        const a = answers(run).map((x) => (/^\d+=[A-F]$/.test(x) ? x.replace("=", "").toLowerCase() : `"${x}"`));
        cmd.textContent = "passo ok" + (a.length ? " " + a.join(" ") : "");
      }
    };
    el.addEventListener("change", refresh);
    el.addEventListener("input", refresh);
    const ok = $("ok");
    if (ok) ok.onclick = () => {
      let list = answers(run);
      if (run.gate.kind === "travado") {
        const h = $("hint").value.trim();
        list = h ? [h] : [];
      }
      if (run.gate.kind === "pergunta") list = list.map((x) => x.replace(/^1=/, ""));
      send({ action: "ok", answers: list });
    };
    const no = $("no");
    if (no) no.onclick = () => $("reject").classList.toggle("open");
    const sendNo = $("send-no");
    if (sendNo) sendNo.onclick = () => send({ action: "no", text: $("reason").value.trim() });
    const stop = $("stop");
    if (stop) stop.onclick = () => confirm("Encerrar o run? O trabalho fica no branch.") && send({ action: "stop", text: "encerrado pelo painel" });
    el.querySelectorAll("[data-size]").forEach((b) => (b.onclick = () => send({ action: "size", text: b.dataset.size })));
  }

  async function send(body) {
    if (!live) return;
    const err = $("err");
    document.querySelectorAll("#now button").forEach((b) => (b.disabled = true));
    try {
      const r = await fetch("/api/decide", { method: "POST", headers: { "Content-Type": "application/json", "X-Passo-Token": boot.token }, body: JSON.stringify(body) });
      const j = await r.json();
      if (!j.ok) throw new Error(j.error || "falhou");
      gateKey = "";
      await load();
    } catch (e) {
      if (err) err.textContent = e.message;
      document.querySelectorAll("#now button").forEach((b) => (b.disabled = false));
    }
  }

  // ---------------------------------------------------------------------------------------------- request & plan
  function request(s) {
    const run = s.run;
    if (!run) return ($("request").innerHTML = "");
    const b = run.brief;
    const li = (xs) => `<ul>${xs.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>`;
    const decided = run.questions.filter((q) => q.answer);
    const changes = run.changes.length
      ? `<h2 style="margin-top:18px">Mudanças no pedido</h2>${run.changes.map((c) => `<div class="change"><div class="k">${time(c.at)} · ${esc(KIND[c.kind] || c.kind)} · ${esc(c.by === "dev" ? "dev" : c.by === "agent" ? "agente" : "passo")}</div><div>${esc(c.text)}</div>${c.before && c.after && c.before !== c.after && c.kind === "resposta" ? `<div class="ba"><s>${esc(c.before)}</s> → ${esc(c.after)}</div>` : ""}</div>`).join("")}`
      : "";
    $("request").innerHTML = `<h2>Pedido · ${esc(SIZE[run.size])}</h2><h3>${esc(run.request)}</h3>
      ${b ? `<dl class="kv"><dt>Entendimento</dt><dd>${esc(b.understanding)}</dd>${b.scope.length ? `<dt>Escopo</dt><dd>${li(b.scope)}</dd>` : ""}${b.out_of_scope.length ? `<dt>Fora do escopo</dt><dd>${li(b.out_of_scope)}</dd>` : ""}${b.assumptions.length ? `<dt>Suposições</dt><dd>${li(b.assumptions)}</dd>` : ""}${decided.length ? `<dt>Decisões</dt><dd>${li(decided.map((q) => q.ask + " → " + (q.options.find((o) => o.id === q.answer)?.text || q.answer)))}</dd>` : ""}</dl>` : ""}
      ${changes}`;
  }

  function plan(s) {
    const run = s.run;
    if (!run || (!run.steps.length && !run.criteria.length)) return ($("plan").innerHTML = "");
    const mark = { done: ["✓", "ok"], doing: ["▸", "doing"], todo: ["·", ""] };
    const by = (id) => {
      const who = run.steps.filter((x) => (x.criteria || []).includes(id)).map((x) => x.id + (x.commit ? " " + x.commit.slice(0, 7) : ""));
      return who.length ? `<div class="meta">← ${esc(who.join(", "))}</div>` : "";
    };
    const steps = run.steps.map((x) => `<li><span class="mark ${mark[x.status][1]}">${mark[x.status][0]}</span><span class="id">${esc(x.id)}</span><span class="body">${esc(x.title)}${x.fix ? ' <span class="tag">ajuste</span>' : ""}${x.tier ? ` <span class="tag">${esc(x.tier)}</span>` : ""}${x.criteria && x.criteria.length ? `<div class="meta">atende ${esc(x.criteria.join(", "))}</div>` : ""}${x.test_command ? `<div class="meta mono">o Passo roda: ${esc(x.test_command)}</div>` : ""}${x.summary ? `<div class="meta">${esc(x.summary)}</div>` : ""}</span>${x.commit ? `<span class="hash">${esc(x.commit.slice(0, 7))}</span>` : ""}</li>`).join("");
    const crit = run.criteria.map((c) => `<li><span class="mark ${c.ok === true ? "ok" : c.ok === false ? "bad" : ""}">${c.ok === true ? "✓" : c.ok === false ? "✗" : "·"}</span><span class="id">${esc(c.id)}</span><span class="body">${esc(c.text)}${by(c.id)}${c.evidence ? `<div class="meta">${esc(c.evidence)}</div>` : ""}</span></li>`).join("");
    $("plan").innerHTML = `<div class="cols"><div><h2>Passos</h2><ul class="list">${steps}</ul></div><div><h2>Critérios de aceite</h2><ul class="list">${crit}</ul></div></div>`;
  }

  function checks(s) {
    const run = s.run;
    if (!run || !run.checks.length) {
      const conf = [...s.checks_step.map((c) => "por passo · " + c), ...s.checks_final.map((c) => "final · " + c)];
      return ($("checks").innerHTML = conf.length ? `<h2>Checagens</h2><p class="small muted">${conf.map(esc).join("<br>")}</p>` : "");
    }
    $("checks").innerHTML = `<h2>Últimas checagens</h2>${run.checks.map((c) => `<details class="check" ${c.ok ? "" : "open"}><summary><span class="${c.ok ? "ok" : "bad"}" style="color:var(--${c.ok ? "ok" : "fail"})">${c.ok ? "✓" : "✗"}</span><code>${esc(c.command)}</code><span class="faint small">${ms(c.ms)}</span></summary>${c.output ? `<pre>${esc(c.output)}</pre>` : ""}</details>`).join("")}`;
  }

  // ---------------------------------------------------------------------------------------------- metrics & activity
  function metrics(s) {
    const m = s.metrics;
    if (!m) return ($("metrics").innerHTML = "");
    const max = Math.max(1, ...m.stages.map((x) => (x.tokens.calls ? billable(x.tokens) : x.ms)));
    const useTokens = m.tokens.calls > 0;
    const bars = m.stages
      .map((x, i) => {
        const v = useTokens ? billable(x.tokens) : x.ms;
        return `<div class="bar"><span>${LABEL[STAGES[i]]}</span><span class="track"><span class="fill" style="width:${(100 * v) / max}%"></span></span><span class="val">${useTokens ? num(v) : x.visits ? ms(v) : "—"}</span></div>`;
      })
      .join("");
    const models = Object.entries(m.by_model).map(([k, t]) => `${esc(k)} · ${num(billable(t))} em ${t.calls} chamadas`).join("<br>");
    $("metrics").innerHTML = `<h2>Métricas</h2><div class="stats">
      <div class="stat"><div class="v">${ms(m.total_ms)}</div><div class="l">tempo total</div></div>
      <div class="stat"><div class="v">${m.dev_turns}</div><div class="l">vez(es) do dev · ${ms(m.dev_wait_ms)} esperando</div></div>
      <div class="stat"><div class="v">${useTokens ? num(billable(m.tokens)) : "—"}</div><div class="l">tokens${useTokens ? " · " + num(m.tokens.cache_read) + " de cache" : " (sem transcrição)"}</div></div>
      <div class="stat"><div class="v">${m.steps_failed}</div><div class="l">tentativas que falharam · ${m.invalid_outputs} entregas inválidas</div></div>
      <div class="stat"><div class="v">${m.tool_calls}</div><div class="l">ações do agente · ${m.lock_denied} negadas</div></div>
      <div class="stat"><div class="v">${m.changes}</div><div class="l">mudanças · plano v${m.plan_versions}</div></div>
      </div><div class="bars"><h2 style="margin-top:6px">${useTokens ? "Tokens" : "Tempo"} por etapa</h2>${bars}</div>${models ? `<div class="models">${models}</div>` : ""}`;
  }

  function activity(s) {
    const pick = {
      tudo: () => true,
      dev: (e) => e.actor === "dev" || e.kind === "gate.opened",
      agente: (e) => e.actor === "agent",
      passo: (e) => e.actor === "passo",
    }[filter];
    const evs = s.events.filter(pick).slice(-200).reverse();
    const chips = ["tudo", "dev", "agente", "passo"].map((f) => `<button class="chip ${f === filter ? "on" : ""}" data-f="${f}">${f}</button>`).join("");
    $("activity").innerHTML = `<h2>Atividade</h2><div class="chips">${chips}</div><div class="events">${
      evs.length
        ? evs.map((e) => {
            const cls = [e.actor, e.kind === "tool" ? "tool" : "", e.kind === "lock.denied" || e.kind === "lock.breach" ? "denied" : ""].join(" ");
            const via = e.data && e.data.via ? `<span class="via">${esc(e.data.via)}</span>` : "";
            return `<div class="ev ${cls}" title="${esc(e.kind)} · ${esc(LABEL[e.stage])}"><span class="t">${time(e.t).slice(0, 5)}</span><span class="dot"></span><span class="x">${esc(e.text)}${via}</span></div>`;
          }).join("")
        : '<p class="faint small">Nada ainda.</p>'
    }</div>`;
    $("activity").querySelectorAll(".chip").forEach((c) => (c.onclick = () => { filter = c.dataset.f; activity(state); }));
  }

  function chrome(s) {
    $("project").textContent = s.project;
    const sel = $("runs");
    const options = s.runs.map((r) => `<option value="${esc(r.id)}">${esc(r.request.slice(0, 48))} · ${esc(r.status === "active" ? LABEL[r.stage] : r.status === "done" ? "concluído" : "interrompido")}</option>`).join("");
    if (sel.dataset.sig !== options) {
      sel.innerHTML = options || "<option>sem runs</option>";
      sel.dataset.sig = options;
    }
    if (s.run) sel.value = s.run.id;
    sel.disabled = !live;
    const audit = s.audit ? ("Ok" in s.audit ? `✓ auditoria íntegra · ${s.audit.Ok} eventos` : `✗ auditoria quebrada: ${esc(s.audit.Err)}`) : "";
    $("foot").innerHTML = `<span>${audit}</span><span>${s.platforms.length ? esc(s.platforms.join(" · ")) : ""}</span><span>${live ? "ao vivo" : "relatório gerado em " + new Date(s.generated_at).toLocaleString("pt-BR")}</span>`;
    document.title = s.run ? `${s.run.gate ? "● " : ""}${LABEL[s.run.stage]} · passo` : "passo";
  }

  function render(s) {
    state = s;
    chrome(s);
    rail(s);
    now(s);
    request(s);
    plan(s);
    checks(s);
    metrics(s);
    activity(s);
  }

  async function load() {
    try {
      const r = await fetch("/api/state" + (selected ? "?run=" + encodeURIComponent(selected) : ""), { cache: "no-store" });
      const s = await r.json();
      if (s.error) throw new Error(s.error);
      $("live").className = "live on";
      $("live").textContent = "ao vivo";
      render(s);
    } catch (e) {
      $("live").className = "live off";
      $("live").textContent = "sem conexão";
    }
  }

  if (live) {
    $("runs").onchange = (e) => { selected = e.target.value; gateKey = ""; load(); };
    load();
    setInterval(load, 1500);
  } else {
    $("live").textContent = "relatório";
    render(boot.snapshot);
  }
})();
