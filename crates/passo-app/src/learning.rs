//! `passo learn`: what the dev keeps correcting across runs becomes a short convention. Passo collects the lessons
//! (no model), an agent proposes sentences, and the dev picks which ones go to `context.md`, the map or an agent.
//! Like `passo map`: read-only, no branch, no stages.

use std::collections::BTreeSet;

use anyhow::{Result, bail};
use passo_core::learn::{self, Lesson};
use passo_core::spec::{self, LEARN_TEMPLATE, LearnTarget, Proposal};
use passo_store::learn::{self as store_learn, LearnTask};
use passo_store::map as store_map;

use crate::service::{App, DoneInput, Rejected, globs, read_input};

/// The section learned lines go under, in `context.md` and in an agent's file.
pub const LEARNED: &str = "## Aprendido com os runs";

/// The trigger: this many new lessons, from this many runs.
const HINT_LESSONS: usize = 3;
const HINT_RUNS: usize = 2;

impl App {
    /// Whether an agent is working on a learning task (delivered proposals wait on the dev, not the agent).
    pub fn learning(&self) -> bool {
        store_learn::load_task(&self.layout).is_some_and(|t| !t.delivered())
    }

    /// Lessons from the finished runs that nobody decided on yet.
    pub fn lessons(&self) -> Vec<Lesson> {
        learn::collect(&self.store.list(), &store_learn::load_seen(&self.layout))
    }

    /// `Há 4 lições novas de 3 runs; …` when there are enough to be worth a look.
    pub fn learn_hint(&self) -> Option<String> {
        if store_learn::load_task(&self.layout).is_some() {
            return None;
        }
        let lessons = self.lessons();
        let runs: BTreeSet<&str> = lessons.iter().map(|l| l.run_id.as_str()).collect();
        (lessons.len() >= HINT_LESSONS && runs.len() >= HINT_RUNS).then(|| {
            format!(
                "Há {} lições novas de {} runs; peça ao agente para rodar passo learn.",
                lessons.len(),
                runs.len()
            )
        })
    }

    pub fn learn_start(&self) -> Result<String> {
        if let Some(run) = self.store.active() {
            return Err(Rejected(format!(
                "termine ou interrompa o run antes de aprender com os runs ({})",
                run.headline()
            ))
            .into());
        }
        if self.mapping() {
            return Err(Rejected(
                "há um mapeamento em andamento; entregue o mapa com passo done ou cancele com passo map --cancel"
                    .into(),
            )
            .into());
        }
        if let Some(task) = store_learn::load_task(&self.layout) {
            return Ok(self.learn_text(&task));
        }
        let lessons = self.lessons();
        if lessons.is_empty() {
            bail!(
                "não há lições novas: nenhuma recusa, mudança, dica ou resposta diferente da recomendada desde a última vez"
            );
        }
        self.store.init()?;
        let task = LearnTask {
            started_at: self.now(),
            lessons,
            proposals: vec![],
            stops: 0,
        };
        store_learn::save_task(&self.layout, &task)?;
        Ok(self.learn_text(&task))
    }

    /// The agent's task, or, once delivered, the proposals the dev decides on.
    pub fn learn_text(&self, task: &LearnTask) -> String {
        if task.delivered() {
            return crate::render::learn_proposals(&task.proposals, &self.bin);
        }
        let map = store_map::load(&self.layout).ok().flatten();
        let files: Vec<&String> = task.lessons.iter().flat_map(|l| &l.files).collect();
        let areas: Vec<&passo_core::map::Node> = map
            .as_ref()
            .map(|m| {
                m.areas
                    .iter()
                    .filter(|a| !a.attached())
                    .filter(|a| globs(&a.paths).is_ok_and(|set| files.iter().any(|f| set.is_match(f))))
                    .collect()
            })
            .unwrap_or_default();
        let all_areas: Vec<String> = map
            .as_ref()
            .map(|m| m.areas.iter().filter(|a| !a.attached()).map(|a| a.id.clone()).collect())
            .unwrap_or_default();
        let agent = self.catalog.agents.get("refinador");
        let (cheap, strong) = (passo_core::config::TIERS[0], passo_core::config::TIERS[1]);
        let delegate = match agent {
            Some(a) if a.model == cheap => format!("refinador-{strong}"),
            _ => "refinador".to_string(),
        };
        crate::render::learn_task(&crate::render::LearnView {
            agent,
            delegation: &self.delegation(&delegate),
            groups: &learn::group(&task.lessons),
            context: self.catalog.context.as_deref(),
            areas: &areas,
            all_areas: &all_areas,
            agents: &self.catalog.agents.keys().cloned().collect::<Vec<_>>(),
            template: LEARN_TEMPLATE,
            bin: &self.bin,
        })
    }

    pub fn learn_done(&self, input: DoneInput) -> Result<String> {
        let Some(mut task) = store_learn::load_task(&self.layout) else {
            bail!("não há aprendizado em andamento; comece com passo learn");
        };
        if task.delivered() {
            return Err(Rejected(format!(
                "as propostas já foram entregues e esperam o dev.\n\n{}",
                self.learn_text(&task)
            ))
            .into());
        }
        let text = read_input(&input.text)?;
        let areas: Vec<String> = store_map::load(&self.layout)
            .ok()
            .flatten()
            .map(|m| m.areas.iter().filter(|a| !a.attached()).map(|a| a.id.clone()).collect())
            .unwrap_or_default();
        let agents: Vec<String> = self.catalog.agents.keys().cloned().collect();
        let proposals = spec::parse_learn(&text, &task.lessons, &areas, &agents).map_err(|invalid| {
            Rejected(format!(
                "As propostas têm problemas:\n{invalid}\nCorrija só esses pontos e rode `passo done` de novo."
            ))
        })?;
        task.proposals = proposals;
        store_learn::save_task(&self.layout, &task)?;
        Ok(format!(
            "{}\nMostre a lista ao dev e pare: ele decide.",
            crate::render::learn_proposals(&task.proposals, &self.bin)
        ))
    }

    /// The dev applies the chosen proposals (`1,3`). Every lesson shown in the task counts as seen afterwards.
    pub fn learn_apply(&self, choice: &str) -> Result<String> {
        let Some(task) = store_learn::load_task(&self.layout).filter(LearnTask::delivered) else {
            bail!("não há propostas esperando o dev; o agente roda passo learn e entrega com passo done");
        };
        let picked = pick(choice, task.proposals.len())?;
        let mut done = vec![];
        let mut agent_changed = false;
        for &n in &picked {
            let proposal = &task.proposals[n - 1];
            match &proposal.target {
                LearnTarget::Context => append_learned(&self.layout.context(), None, proposal)?,
                LearnTarget::Mapa(area) => {
                    let Some(mut map) = store_map::load(&self.layout)? else {
                        bail!("a proposta {n} vai para o mapa, mas o projeto não tem mapa");
                    };
                    let Some(node) = map.areas.iter_mut().find(|a| &a.id == area) else {
                        bail!("a proposta {n} vai para a área `{area}`, que saiu do mapa");
                    };
                    crate::mapping::annotate(node, "aprendido", &proposal.text);
                    store_map::save(&self.layout, &map)?;
                }
                LearnTarget::Agente(agent) => {
                    let path = self.layout.agents().join(format!("{agent}.md"));
                    append_learned(&path, Some(agent), proposal)?;
                    agent_changed = true;
                }
            }
            done.push(format!("{n}. {} → {}", proposal.text, proposal.target.label()));
        }
        let prints: Vec<String> = task.lessons.iter().map(|l| l.fingerprint.clone()).collect();
        store_learn::add_seen(&self.layout, &prints)?;
        store_learn::remove_task(&self.layout);
        let mut out = if done.is_empty() {
            "Nenhuma proposta aplicada.".to_string()
        } else {
            format!("Aplicadas:\n{}", done.join("\n"))
        };
        out.push_str("\nAs lições desta rodada não voltam a aparecer. Commite .passo/ (context.md, map/, agents/, learn/seen.json).");
        if agent_changed {
            out.push_str("\nUm agente mudou: rode passo sync para a plataforma ler a nova versão.");
        }
        Ok(out)
    }

    /// The dev drops the learning in progress; its lessons stay new.
    pub fn learn_cancel(&self) -> Result<String> {
        if store_learn::load_task(&self.layout).is_none() {
            bail!("não há aprendizado em andamento");
        }
        store_learn::remove_task(&self.layout);
        Ok("Aprendizado descartado.".into())
    }

    /// Counts a stop the hook blocked while the agent works on the proposals; the hook gives up after a few.
    pub fn learn_count_stop(&self) -> u32 {
        let Some(mut task) = store_learn::load_task(&self.layout) else {
            return u32::MAX;
        };
        task.stops += 1;
        store_learn::save_task(&self.layout, &task).ok();
        task.stops
    }
}

/// `1,3`, `1 3` or `todas`: proposal numbers, 1-based, each once.
fn pick(choice: &str, count: usize) -> Result<Vec<usize>> {
    let choice = choice.trim();
    if matches!(choice, "todas" | "all") {
        return Ok((1..=count).collect());
    }
    if matches!(choice, "nenhuma" | "none") {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for part in choice.split([',', ' ']).filter(|p| !p.trim().is_empty()) {
        match part.trim().parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            _ => {
                return Err(Rejected(format!(
                    "`{part}` não é uma proposta; use números de 1 a {count} (ex.: passo learn --apply 1,3), todas ou nenhuma"
                ))
                .into());
            }
        }
    }
    if out.is_empty() {
        return Err(Rejected("diga quais propostas aplicar: passo learn --apply 1,3".into()).into());
    }
    Ok(out)
}

/// Adds `- <text> <!-- fontes: … -->` at the end of the learned section, creating the file or the section.
fn append_learned(path: &std::path::Path, extends: Option<&str>, proposal: &Proposal) -> Result<()> {
    let sources = proposal.sources.join("; ").replace("--", "—");
    let line = format!("- {} <!-- fontes: {sources} -->", proposal.text);
    let current = std::fs::read_to_string(path).unwrap_or_default();
    let text = if current.trim().is_empty() {
        let head = match extends {
            Some(agent) => format!("---\nextends: {agent}\n---\n"),
            None => "# Convenções do projeto\n".to_string(),
        };
        format!("{head}\n{LEARNED}\n{line}\n")
    } else if let Some(at) = current.find(LEARNED) {
        // The section ends at the next heading of the same level, or at the end of the file.
        let body_start = at + LEARNED.len();
        let end = current[body_start..]
            .find("\n## ")
            .map(|i| body_start + i)
            .unwrap_or(current.len());
        let (before, after) = current.split_at(end);
        let after = after.trim_start_matches('\n');
        if after.is_empty() {
            format!("{}\n{line}\n", before.trim_end())
        } else {
            format!("{}\n{line}\n\n{after}", before.trim_end())
        }
    } else {
        format!("{}\n\n{LEARNED}\n{line}\n", current.trim_end())
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    passo_store::layout::write_atomic(path, text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_numbers_once_and_refuses_the_rest() {
        assert_eq!(pick("1,3", 3).unwrap(), vec![1, 3]);
        assert_eq!(pick("2 2", 3).unwrap(), vec![2]);
        assert_eq!(pick("todas", 2).unwrap(), vec![1, 2]);
        assert!(pick("4", 3).is_err());
        assert!(pick("", 3).is_err());
    }

    #[test]
    fn learned_lines_go_to_the_end_of_their_section() {
        let dir = std::env::temp_dir().join(format!("passo-learned-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("context.md");
        std::fs::write(
            &path,
            "# Convenções\n\n## Aprendido com os runs\n- antiga\n\n## Outra\n- x\n",
        )
        .unwrap();
        let proposal = Proposal {
            text: "nova".into(),
            target: LearnTarget::Context,
            sources: vec!["run r1: recusa: y".into()],
            lessons: vec![],
        };
        append_learned(&path, None, &proposal).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("- antiga\n- nova <!-- fontes: run r1: recusa: y -->\n\n## Outra"),
            "{text}"
        );
        let agent = dir.join("agents/construtor.md");
        append_learned(&agent, Some("construtor"), &proposal).unwrap();
        assert!(
            std::fs::read_to_string(&agent)
                .unwrap()
                .starts_with("---\nextends: construtor\n---\n")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
