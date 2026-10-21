//! What agents hand in: the refined request (brief), the plan and the review.
//!
//! Agents write JSON; Passo parses it leniently (code fences and chatter around it are fine) and validates it
//! strictly, answering with one line per problem. A cheap model fixes a precise error; it does not fix "invalid".

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub ask: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    pub options: Vec<QuestionOption>,
    /// Id of the recommended option.
    pub recommended: String,
    /// An option id, or the dev's own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

impl Question {
    /// The answer as text the agent can follow.
    pub fn answer_text(&self) -> Option<String> {
        let answer = self.answer.as_ref()?;
        Some(match self.options.iter().find(|o| &o.id == answer) {
            Some(option) => option.text.clone(),
            None => answer.clone(),
        })
    }

    pub fn recommended_text(&self) -> &str {
        self.options
            .iter()
            .find(|o| o.id == self.recommended)
            .map(|o| o.text.as_str())
            .unwrap_or("")
    }
}

/// The refined request: what the dev wants, in the agent's words, and what is still open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    pub understanding: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<crate::model::Size>,
    pub scope: Vec<String>,
    #[serde(default)]
    pub out_of_scope: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub test: String,
    /// Ids of the criteria this step meets (`C1`, `C2`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<String>,
    /// The narrowest command that runs this step's test (`rodar`); Passo runs it on `done`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub test_command: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub title: String,
    pub criteria: Vec<String>,
    pub steps: Vec<PlanStep>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub id: String,
    pub ok: bool,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub criteria: Vec<ReviewItem>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// One line per problem, in the words the agent should read.
#[derive(Debug, Clone, PartialEq)]
pub struct Invalid(pub Vec<String>);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for line in &self.0 {
            writeln!(f, "- {line}")?;
        }
        Ok(())
    }
}

/// Finds the JSON object in what an agent wrote: plain, inside a ```json fence, or with words around it.
pub fn extract_json(text: &str) -> Result<Value, Invalid> {
    let start = text.find('{');
    let end = text.rfind('}');
    let (Some(start), Some(end)) = (start, end) else {
        return Err(Invalid(vec![
            "não achei um objeto JSON ({ … }) no que foi entregue".into(),
        ]));
    };
    if end < start {
        return Err(Invalid(vec!["o JSON entregue está incompleto".into()]));
    }
    serde_json::from_str(&text[start..=end]).map_err(|e| {
        Invalid(vec![format!(
            "JSON inválido na linha {}, coluna {}: {}",
            e.line(),
            e.column(),
            e
        )])
    })
}

struct Reader<'a> {
    root: &'a Value,
    errors: Vec<String>,
}

impl<'a> Reader<'a> {
    fn new(root: &'a Value) -> Self {
        Reader { root, errors: vec![] }
    }

    fn text(&mut self, at: &Value, path: &str, key: &str, required: bool) -> String {
        match at.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
            Some(Value::String(_)) | None | Some(Value::Null) => {
                if required {
                    self.errors
                        .push(format!("`{}` é obrigatório e não pode ficar vazio", join(path, key)));
                }
                String::new()
            }
            Some(_) => {
                self.errors.push(format!("`{}` deve ser um texto", join(path, key)));
                String::new()
            }
        }
    }

    fn list(&mut self, at: &Value, path: &str, key: &str, required: bool) -> Vec<String> {
        match at.get(key) {
            Some(Value::Array(items)) => {
                let mut out = vec![];
                for (i, item) in items.iter().enumerate() {
                    match item {
                        Value::String(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
                        Value::String(_) => {}
                        _ => self
                            .errors
                            .push(format!("`{}[{i}]` deve ser um texto", join(path, key))),
                    }
                }
                if required && out.is_empty() {
                    self.errors
                        .push(format!("`{}` precisa de pelo menos um item", join(path, key)));
                }
                out
            }
            None | Some(Value::Null) => {
                if required {
                    self.errors
                        .push(format!("`{}` é obrigatório (uma lista de textos)", join(path, key)));
                }
                vec![]
            }
            Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
            Some(_) => {
                self.errors
                    .push(format!("`{}` deve ser uma lista de textos", join(path, key)));
                vec![]
            }
        }
    }

    fn objects<'v>(&mut self, at: &'v Value, path: &str, key: &str, required: bool) -> Vec<&'v Value> {
        match at.get(key) {
            Some(Value::Array(items)) => {
                let objects: Vec<&Value> = items.iter().filter(|v| v.is_object()).collect();
                if objects.len() != items.len() {
                    self.errors
                        .push(format!("cada item de `{}` deve ser um objeto {{ … }}", join(path, key)));
                }
                if required && objects.is_empty() {
                    self.errors
                        .push(format!("`{}` precisa de pelo menos um item", join(path, key)));
                }
                objects
            }
            None | Some(Value::Null) => {
                if required {
                    self.errors
                        .push(format!("`{}` é obrigatório (uma lista de objetos)", join(path, key)));
                }
                vec![]
            }
            Some(_) => {
                self.errors
                    .push(format!("`{}` deve ser uma lista de objetos", join(path, key)));
                vec![]
            }
        }
    }

    fn finish<T>(self, value: T) -> Result<T, Invalid> {
        if self.errors.is_empty() {
            Ok(value)
        } else {
            Err(Invalid(self.errors))
        }
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

const LETTERS: [&str; 6] = ["A", "B", "C", "D", "E", "F"];

/// Parses `perguntas`. `first` is the number of the first question (questions keep growing during a run).
fn questions(r: &mut Reader, at: &Value, first: usize) -> Vec<Question> {
    let mut out = vec![];
    for (i, q) in r.objects(at, "", "perguntas", false).into_iter().enumerate() {
        let path = format!("perguntas[{i}]");
        let ask = r.text(q, &path, "pergunta", true);
        let why = r.text(q, &path, "porque", false);
        let raw = r.objects(q, &path, "opcoes", true);
        if !raw.is_empty() && !(2..=LETTERS.len()).contains(&raw.len()) {
            r.errors
                .push(format!("`{path}.opcoes` deve ter de 2 a {} opções", LETTERS.len()));
        }
        let options: Vec<QuestionOption> = raw
            .iter()
            .take(LETTERS.len())
            .enumerate()
            .map(|(j, o)| QuestionOption {
                id: LETTERS[j].to_string(),
                text: r.text(o, &format!("{path}.opcoes[{j}]"), "texto", true),
                why: r.text(o, &format!("{path}.opcoes[{j}]"), "porque", false),
            })
            .collect();
        let recommended = match q.get("recomendada") {
            None | Some(Value::Null) => 1,
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
            Some(Value::String(s)) => match LETTERS.iter().position(|l| l.eq_ignore_ascii_case(s.trim())) {
                Some(p) => p + 1,
                None => s.trim().parse().unwrap_or(0),
            },
            Some(_) => 0,
        };
        if recommended == 0 || recommended > options.len().max(1) {
            r.errors.push(format!(
                "`{path}.recomendada` deve ser o número de uma das opções (1 = a primeira)"
            ));
        }
        out.push(Question {
            id: format!("Q{}", first + i),
            ask,
            why,
            recommended: LETTERS.get(recommended.saturating_sub(1)).unwrap_or(&"A").to_string(),
            options,
            answer: None,
        });
    }
    out
}

pub fn parse_brief(text: &str, first_question: usize) -> Result<Brief, Invalid> {
    let root = extract_json(text)?;
    let mut r = Reader::new(&root);
    if !root.is_object() {
        return Err(Invalid(vec!["o pedido refinado deve ser um objeto JSON".into()]));
    }
    let understanding = r.text(r.root, "", "entendimento", true);
    let size = match root.get("tamanho") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => match crate::model::Size::parse(s) {
            Some(size) => Some(size),
            None => {
                r.errors.push("`tamanho` deve ser pp, p, m ou g".into());
                None
            }
        },
        Some(_) => {
            r.errors.push("`tamanho` deve ser pp, p, m ou g".into());
            None
        }
    };
    let brief = Brief {
        understanding,
        size,
        scope: r.list(&root, "", "escopo", true),
        out_of_scope: r.list(&root, "", "fora_do_escopo", false),
        assumptions: r.list(&root, "", "suposicoes", false),
        files: r.list(&root, "", "arquivos", false),
        questions: questions(&mut r, &root, first_question),
    };
    r.finish(brief)
}

pub fn parse_plan(text: &str) -> Result<Plan, Invalid> {
    let root = extract_json(text)?;
    if !root.is_object() {
        return Err(Invalid(vec!["o plano deve ser um objeto JSON".into()]));
    }
    let mut r = Reader::new(&root);
    let title = r.text(&root, "", "titulo", true);
    let criteria = r.list(&root, "", "criterios", true);
    let mut steps = vec![];
    for (i, s) in r.objects(&root, "", "passos", true).into_iter().enumerate() {
        let path = format!("passos[{i}]");
        steps.push(PlanStep {
            title: r.text(s, &path, "titulo", true),
            detail: r.text(s, &path, "detalhe", false),
            files: r.list(s, &path, "arquivos", false),
            test: r.text(s, &path, "teste", false),
            criteria: criterion_refs(&mut r, s, &path, criteria.len()),
            test_command: r.text(s, &path, "rodar", false),
        });
    }
    let notes = r.text(&root, "", "notas", false);
    r.finish(Plan {
        title,
        criteria,
        steps,
        notes,
    })
}

/// Reads a step's `criterios`: `"C1"`, `"c1"`, `"1"` or `1` all mean C1. A reference past the plan's criteria is
/// an error that says how many there are.
fn criterion_refs(r: &mut Reader, at: &Value, path: &str, count: usize) -> Vec<String> {
    let items = match at.get("criterios") {
        None | Some(Value::Null) => return vec![],
        Some(Value::Array(items)) => items.clone(),
        Some(other) => vec![other.clone()],
    };
    let mut out = vec![];
    for item in items {
        let number = match &item {
            Value::Number(n) => n.as_u64().map(|n| n as usize),
            Value::String(s) => s.trim().trim_start_matches(['C', 'c']).parse().ok(),
            _ => None,
        };
        match number {
            Some(n) if (1..=count).contains(&n) => {
                let id = format!("C{n}");
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            Some(n) => r
                .errors
                .push(format!("`{path}.criterios` cita C{n}, mas há {count} critério(s)")),
            None => r
                .errors
                .push(format!("`{path}.criterios` deve listar ids de critérios, como \"C1\"")),
        }
    }
    out
}

/// Parses the review and checks that it answers every criterion, and nothing else.
pub fn parse_review(text: &str, criteria: &[String]) -> Result<Review, Invalid> {
    let root = extract_json(text)?;
    if !root.is_object() {
        return Err(Invalid(vec!["a revisão deve ser um objeto JSON".into()]));
    }
    let mut r = Reader::new(&root);
    let mut items = vec![];
    for (i, c) in r.objects(&root, "", "criterios", true).into_iter().enumerate() {
        let path = format!("criterios[{i}]");
        let id = r.text(c, &path, "id", true).to_uppercase();
        let ok = match c.get("ok") {
            Some(Value::Bool(b)) => *b,
            _ => {
                r.errors.push(format!("`{path}.ok` deve ser true ou false"));
                false
            }
        };
        let evidence = r.text(c, &path, "evidencia", true);
        items.push(ReviewItem { id, ok, evidence });
    }
    let notes = r.list(&root, "", "observacoes", false);
    for id in criteria {
        if !items.iter().any(|i| &i.id == id) {
            r.errors.push(format!("falta o critério {id} em `criterios`"));
        }
    }
    for item in &items {
        if !item.id.is_empty() && !criteria.contains(&item.id) {
            r.errors.push(format!(
                "o critério {} não existe; use só {}",
                item.id,
                criteria.join(", ")
            ));
        }
    }
    r.finish(Review { criteria: items, notes })
}

/// Something a review cites as proof: a path (maybe with a line), or a test name in backticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRef {
    pub path: Option<String>,
    /// The last line cited (`src/x.rs:42-50` cites up to 50).
    pub line: Option<u32>,
    pub symbol: Option<String>,
}

/// Picks the candidates out of an evidence text. Only the form is read here: whether a candidate is really a path
/// is for the disk to say, so anything with a slash or a file extension is a candidate.
pub fn evidence_refs(text: &str) -> Vec<EvidenceRef> {
    let mut refs = vec![];
    let mut rest = String::new();
    let mut quoted = text.split('`');
    if let Some(first) = quoted.next() {
        rest.push_str(first);
    }
    // Odd pieces sit between backticks.
    for (i, piece) in quoted.enumerate() {
        if i % 2 == 0 {
            let inner = piece.trim();
            match path_ref(inner) {
                Some(path) => refs.push(path),
                None if !inner.is_empty() => refs.push(EvidenceRef {
                    path: None,
                    line: None,
                    symbol: Some(inner.to_string()),
                }),
                None => {}
            }
        } else {
            rest.push(' ');
            rest.push_str(piece);
        }
    }
    for word in rest.split(|c: char| c.is_whitespace() || ";,()[]\"'".contains(c)) {
        if let Some(path) = path_ref(word) {
            refs.push(path);
        }
    }
    refs
}

fn path_ref(word: &str) -> Option<EvidenceRef> {
    let word = word.trim_end_matches([':', '.', '!', '?']);
    if word.is_empty() || word.contains("://") || word.contains(char::is_whitespace) {
        return None;
    }
    let (path, line) = match word.split_once(':') {
        Some((path, spec)) => {
            let last = spec.rsplit('-').next().unwrap_or(spec);
            match last.parse::<u32>() {
                Ok(n) if spec.split('-').all(|p| p.parse::<u32>().is_ok()) => (path, Some(n)),
                _ => (path, None),
            }
        }
        None => (word, None),
    };
    let extension = path.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && (1..=6).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    (path.contains('/') || extension).then(|| EvidenceRef {
        path: Some(path.to_string()),
        line,
        symbol: None,
    })
}

/// Parses one question asked mid-run: the first option is the recommended one.
pub fn question_from_cli(id: String, ask: &str, options: &[String]) -> Result<Question, Invalid> {
    let mut errors = vec![];
    if ask.trim().is_empty() {
        errors.push("a pergunta está vazia".to_string());
    }
    if !(2..=LETTERS.len()).contains(&options.len()) {
        errors.push(format!("dê de 2 a {} opções, a recomendada primeiro", LETTERS.len()));
    }
    if !errors.is_empty() {
        return Err(Invalid(errors));
    }
    let options = options
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let (text, why) = match raw.split_once(" — ").or_else(|| raw.split_once(" - ")) {
                Some((t, w)) => (t.trim().to_string(), w.trim().to_string()),
                None => (raw.trim().to_string(), String::new()),
            };
            QuestionOption {
                id: LETTERS[i].to_string(),
                text,
                why,
            }
        })
        .collect();
    Ok(Question {
        id,
        ask: ask.trim().to_string(),
        why: String::new(),
        options,
        recommended: "A".into(),
        answer: None,
    })
}

/// What the agent fills in at Refinar. Kept next to the parser so the two never drift apart.
pub const BRIEF_TEMPLATE: &str = r#"{
  "entendimento": "o que o dev quer, em 1 a 3 frases",
  "tamanho": "m",
  "escopo": ["o que entra"],
  "fora_do_escopo": ["o que fica de fora"],
  "suposicoes": ["o que você assumiu sem perguntar"],
  "arquivos": ["caminho/que/o/pedido/toca"],
  "perguntas": [
    {
      "pergunta": "a dúvida que muda o resultado",
      "porque": "o que muda conforme a resposta",
      "opcoes": [
        { "texto": "opção recomendada", "porque": "por que é a melhor" },
        { "texto": "outra opção", "porque": "quando ela seria melhor" }
      ],
      "recomendada": 1
    }
  ]
}"#;

pub const PLAN_TEMPLATE: &str = r#"{
  "titulo": "nome curto da entrega",
  "criterios": ["comportamento verificável 1", "comportamento verificável 2"],
  "passos": [
    {
      "titulo": "o que este passo faz",
      "detalhe": "como, se ajudar (opcional)",
      "arquivos": ["src/arquivo.ext"],
      "teste": "o teste que prova este passo (vazio se não há o que testar)",
      "criterios": ["C1"],
      "rodar": "o comando mais estreito que roda só esse teste (opcional)"
    }
  ],
  "notas": "decisões de design (opcional)"
}"#;

pub const REVIEW_TEMPLATE: &str = r#"{
  "criterios": [
    { "id": "C1", "ok": true, "evidencia": "tests/x_test.rs:12 `prova_tal`; src/x.rs:42" }
  ],
  "observacoes": ["o que você achou além dos critérios (opcional)"]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_from_fenced_json() {
        let text = "Aqui está:\n```json\n{\"entendimento\":\"x\",\"escopo\":[\"a\"],\"perguntas\":[{\"pergunta\":\"qual?\",\"opcoes\":[{\"texto\":\"um\"},{\"texto\":\"dois\"}],\"recomendada\":2}]}\n```";
        let brief = parse_brief(text, 1).unwrap();
        assert_eq!(brief.questions[0].id, "Q1");
        assert_eq!(brief.questions[0].recommended, "B");
    }

    #[test]
    fn brief_errors_are_precise() {
        let err = parse_brief("{\"escopo\": [], \"tamanho\": \"xl\"}", 1).unwrap_err();
        assert!(err.0.iter().any(|e| e.contains("`entendimento`")));
        assert!(err.0.iter().any(|e| e.contains("`escopo`")));
        assert!(err.0.iter().any(|e| e.contains("`tamanho`")));
    }

    #[test]
    fn plan_requires_steps_and_criteria() {
        let err = parse_plan("{\"titulo\":\"t\",\"criterios\":[],\"passos\":[{\"arquivos\":[\"a\"]}]}").unwrap_err();
        assert!(err.0.iter().any(|e| e.contains("`criterios`")));
        assert!(err.0.iter().any(|e| e.contains("`passos[0].titulo`")));
    }

    #[test]
    fn step_criteria_are_normalized_and_checked() {
        let plan =
            parse_plan(r#"{"titulo":"t","criterios":["a","b"],"passos":[{"titulo":"x","criterios":["c2", 1, "C1"]}]}"#)
                .unwrap();
        assert_eq!(plan.steps[0].criteria, vec!["C2", "C1"]);
        let err = parse_plan(r#"{"titulo":"t","criterios":["a","b","c"],"passos":[{"titulo":"x"},{"titulo":"y"},{"titulo":"z","criterios":["C5"]}]}"#)
            .unwrap_err();
        assert_eq!(
            err.0,
            vec!["`passos[2].criterios` cita C5, mas há 3 critério(s)".to_string()]
        );
    }

    #[test]
    fn evidence_refs_read_paths_lines_ranges_and_tests() {
        let refs = evidence_refs(
            "src/tasks.js:24-30 add(file); tests/x_test.rs: `prioridade padrão` cobre, README.md e src/cli.js:8",
        );
        let paths: Vec<(Option<&str>, Option<u32>)> = refs
            .iter()
            .filter(|r| r.path.is_some())
            .map(|r| (r.path.as_deref(), r.line))
            .collect();
        assert_eq!(
            paths,
            vec![
                (Some("src/tasks.js"), Some(30)),
                (Some("tests/x_test.rs"), None),
                (Some("README.md"), None),
                (Some("src/cli.js"), Some(8))
            ]
        );
        assert!(refs.iter().any(|r| r.symbol.as_deref() == Some("prioridade padrão")));
        assert!(evidence_refs("rodei o comando e funcionou").is_empty());
        assert!(evidence_refs("veja https://exemplo.com/x.html").is_empty());
    }

    #[test]
    fn review_must_cover_every_criterion() {
        let criteria = vec!["C1".to_string(), "C2".to_string()];
        let err = parse_review(
            "{\"criterios\":[{\"id\":\"c1\",\"ok\":true,\"evidencia\":\"teste x\"}]}",
            &criteria,
        )
        .unwrap_err();
        assert_eq!(err.0, vec!["falta o critério C2 em `criterios`".to_string()]);
    }

    #[test]
    fn cli_question_splits_why() {
        let q = question_from_cli("Q3".into(), "Qual?", &["Um — mais simples".into(), "Dois".into()]).unwrap();
        assert_eq!(q.options[0].why, "mais simples");
        assert_eq!(q.recommended, "A");
    }
}
