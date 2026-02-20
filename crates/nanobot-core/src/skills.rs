use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::util::markdown;

/// A skill bundled as a Rust constant (no filesystem needed).
pub struct BundledSkill {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub content: &'static str,
}

/// All categories used by bundled skills.
pub const SKILL_CATEGORIES: &[&str] = &[
    "仕事効率化",
    "プログラミング",
    "クリエイティブ",
    "リサーチ",
    "学習",
    "日常",
];

pub const BUNDLED_SKILLS: &[BundledSkill] = &[
    // 仕事効率化
    BundledSkill {
        id: "email-draft",
        name: "メール下書き",
        description: "ビジネスメールを即座に作成",
        category: "仕事効率化",
        content: "# メール下書きスキル\n\n\
            ユーザーの意図を汲み取り、適切なビジネスメールを作成してください。\n\n\
            ## ルール\n\
            - 宛先・件名・本文を構造化して出力\n\
            - 敬語レベルを相手との関係性に合わせる（社外→丁寧、社内→適度にカジュアル）\n\
            - 箇条書きを活用して読みやすく\n\
            - 締めの挨拶を忘れずに\n\
            - 英語メールの場合も対応可能",
    },
    BundledSkill {
        id: "meeting-notes",
        name: "議事録作成",
        description: "会議メモから構造化された議事録を生成",
        category: "仕事効率化",
        content: "# 議事録作成スキル\n\n\
            会議のメモやテキストから、構造化された議事録を生成してください。\n\n\
            ## 出力フォーマット\n\
            - 日時・参加者・議題\n\
            - 各議題の要約と決定事項\n\
            - アクションアイテム（担当者・期限付き）\n\
            - 次回予定",
    },
    BundledSkill {
        id: "task-breakdown",
        name: "タスク分解",
        description: "大きなタスクを実行可能なステップに分解",
        category: "仕事効率化",
        content: "# タスク分解スキル\n\n\
            大きなタスクや目標を、具体的で実行可能なステップに分解してください。\n\n\
            ## ルール\n\
            - 各ステップは30分〜2時間で完了できる粒度\n\
            - 依存関係を明示（何が先に必要か）\n\
            - 優先度（高/中/低）を付与\n\
            - 所要時間の目安を記載\n\
            - チェックリスト形式で出力",
    },
    // プログラミング
    BundledSkill {
        id: "code-review",
        name: "コードレビュー",
        description: "コードの問題点と改善案を提示",
        category: "プログラミング",
        content: "# コードレビュースキル\n\n\
            提示されたコードをレビューし、問題点と改善案を提示してください。\n\n\
            ## チェック項目\n\
            - バグ・ロジックエラー\n\
            - セキュリティ脆弱性（SQL injection, XSS等）\n\
            - パフォーマンス問題\n\
            - 可読性・命名規則\n\
            - エッジケースの処理漏れ\n\
            - 重要度（Critical/Warning/Info）で分類",
    },
    BundledSkill {
        id: "debug-helper",
        name: "デバッグ支援",
        description: "エラーメッセージから原因と解決策を特定",
        category: "プログラミング",
        content: "# デバッグ支援スキル\n\n\
            エラーメッセージやスタックトレースから原因を特定し、解決策を提示してください。\n\n\
            ## アプローチ\n\
            - エラーメッセージの意味を平易に説明\n\
            - 考えられる原因を可能性順にリスト\n\
            - 各原因に対する具体的な修正コード\n\
            - 再発防止のためのアドバイス",
    },
    BundledSkill {
        id: "api-design",
        name: "API設計",
        description: "RESTful APIのエンドポイント設計を支援",
        category: "プログラミング",
        content: "# API設計スキル\n\n\
            RESTful APIのエンドポイント設計を支援してください。\n\n\
            ## 出力内容\n\
            - エンドポイント一覧（メソッド、パス、説明）\n\
            - リクエスト/レスポンスのJSON例\n\
            - ステータスコードの使い分け\n\
            - 認証・認可の方針\n\
            - ページネーション・フィルタリング設計",
    },
    // クリエイティブ
    BundledSkill {
        id: "blog-writer",
        name: "ブログ執筆",
        description: "SEO最適化されたブログ記事を執筆",
        category: "クリエイティブ",
        content: "# ブログ執筆スキル\n\n\
            SEOを意識した読みやすいブログ記事を執筆してください。\n\n\
            ## ルール\n\
            - 見出し（H2/H3）で構造化\n\
            - 導入→本文→まとめの3部構成\n\
            - キーワードを自然に配置\n\
            - 1段落は3〜4文に収める\n\
            - メタディスクリプション案も併記",
    },
    BundledSkill {
        id: "copy-polish",
        name: "文章推敲",
        description: "文章を校正・推敲して品質向上",
        category: "クリエイティブ",
        content: "# 文章推敲スキル\n\n\
            提示された文章を校正・推敲し、品質を向上させてください。\n\n\
            ## チェック項目\n\
            - 誤字脱字・文法ミス\n\
            - 冗長な表現の簡潔化\n\
            - 文体の統一\n\
            - 論理の流れ・一貫性\n\
            - 修正理由を簡潔に添える",
    },
    // リサーチ
    BundledSkill {
        id: "market-research",
        name: "市場調査",
        description: "競合・市場トレンドを構造化して分析",
        category: "リサーチ",
        content: "# 市場調査スキル\n\n\
            指定された市場・業界について構造化された分析を行ってください。\n\n\
            ## 分析フレームワーク\n\
            - 市場規模と成長率\n\
            - 主要プレイヤーと市場シェア\n\
            - トレンドと今後の展望\n\
            - SWOT分析\n\
            - 参入障壁と機会",
    },
    BundledSkill {
        id: "summarizer",
        name: "要約",
        description: "長文を要点を押さえて簡潔に要約",
        category: "リサーチ",
        content: "# 要約スキル\n\n\
            長文を要点を押さえて簡潔に要約してください。\n\n\
            ## ルール\n\
            - 原文の1/5〜1/3の長さに圧縮\n\
            - 重要なポイントを箇条書きで抽出\n\
            - 数値やデータは正確に保持\n\
            - 著者の主張・結論を明確に\n\
            - 必要に応じて1行サマリーも追加",
    },
    // 学習
    BundledSkill {
        id: "explain-concept",
        name: "概念解説",
        description: "難しい概念を分かりやすく段階的に説明",
        category: "学習",
        content: "# 概念解説スキル\n\n\
            難しい概念を段階的に、分かりやすく説明してください。\n\n\
            ## アプローチ\n\
            - まず一言で簡潔に説明\n\
            - 身近な例えで直感的に理解\n\
            - 詳細な仕組みを段階的に解説\n\
            - 関連する概念との違い\n\
            - 理解度チェック用の質問を1つ添える",
    },
    BundledSkill {
        id: "quiz-maker",
        name: "クイズ作成",
        description: "学習内容からクイズを自動生成",
        category: "学習",
        content: "# クイズ作成スキル\n\n\
            指定されたトピックや学習内容からクイズを生成してください。\n\n\
            ## フォーマット\n\
            - 4択問題を5〜10問\n\
            - 難易度を初級/中級/上級で設定\n\
            - 各問題に解説を付与\n\
            - 正答率の目安を記載\n\
            - 最後に復習ポイントをまとめ",
    },
    // 日常
    BundledSkill {
        id: "travel-plan",
        name: "旅行プラン",
        description: "目的地・日程から旅行計画を作成",
        category: "日常",
        content: "# 旅行プランスキル\n\n\
            目的地と日程から、具体的な旅行計画を作成してください。\n\n\
            ## 出力内容\n\
            - 日ごとのスケジュール（時間付き）\n\
            - おすすめスポットと所要時間\n\
            - 移動手段と所要時間\n\
            - 予算の目安\n\
            - 持ち物リスト・注意事項",
    },
    BundledSkill {
        id: "recipe-suggest",
        name: "レシピ提案",
        description: "食材から作れるレシピを提案",
        category: "日常",
        content: "# レシピ提案スキル\n\n\
            手持ちの食材から作れるレシピを提案してください。\n\n\
            ## 出力内容\n\
            - 料理名と調理時間\n\
            - 材料リスト（分量付き）\n\
            - 手順を番号付きで\n\
            - カロリー・栄養素の目安\n\
            - アレンジや時短のコツ",
    },
    BundledSkill {
        id: "health-log",
        name: "健康記録",
        description: "日々の体調・運動を記録し傾向を分析",
        category: "日常",
        content: "# 健康記録スキル\n\n\
            日々の体調や運動を記録し、傾向を分析してください。\n\n\
            ## 記録項目\n\
            - 体調（1-10スケール）\n\
            - 睡眠時間と質\n\
            - 運動内容と時間\n\
            - 食事の概要\n\
            - 気分・ストレスレベル\n\
            - 週次/月次のトレンド分析",
    },
];

/// Look up a bundled skill by ID.
pub fn get_bundled_skill(id: &str) -> Option<&'static BundledSkill> {
    BUNDLED_SKILLS.iter().find(|s| s.id == id)
}

/// Loader for agent skills.
///
/// Skills are markdown files (SKILL.md) that teach the agent how to use
/// specific tools or perform certain tasks.
pub struct SkillsLoader {
    workspace_skills: PathBuf,
    builtin_skills: Option<PathBuf>,
}

/// Information about a skill.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub path: PathBuf,
    pub source: String,
}

impl SkillsLoader {
    pub fn new(workspace: &Path, builtin_skills: Option<PathBuf>) -> Self {
        Self {
            workspace_skills: workspace.join("skills"),
            builtin_skills,
        }
    }

    /// List all available skills.
    pub fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo> {
        let mut skills = Vec::new();

        // Workspace skills (highest priority)
        if self.workspace_skills.exists() {
            if let Ok(entries) = std::fs::read_dir(&self.workspace_skills) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if dir.is_dir() {
                        let skill_file = dir.join("SKILL.md");
                        if skill_file.exists() {
                            let name = dir
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            skills.push(SkillInfo {
                                name,
                                path: skill_file,
                                source: "workspace".to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Built-in skills
        if let Some(ref builtin_dir) = self.builtin_skills {
            if builtin_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(builtin_dir) {
                    for entry in entries.flatten() {
                        let dir = entry.path();
                        if dir.is_dir() {
                            let name = dir
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            // Skip if workspace already has this skill
                            if skills.iter().any(|s| s.name == name) {
                                continue;
                            }
                            let skill_file = dir.join("SKILL.md");
                            if skill_file.exists() {
                                skills.push(SkillInfo {
                                    name,
                                    path: skill_file,
                                    source: "builtin".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        if filter_unavailable {
            skills.retain(|s| {
                let meta = self.get_skill_nanobot_meta(&s.name);
                check_requirements(&meta)
            });
        }

        skills
    }

    /// Load a skill by name.
    pub fn load_skill(&self, name: &str) -> Option<String> {
        // Check workspace first
        let ws_skill = self.workspace_skills.join(name).join("SKILL.md");
        if ws_skill.exists() {
            return std::fs::read_to_string(&ws_skill).ok();
        }

        // Check built-in
        if let Some(ref builtin_dir) = self.builtin_skills {
            let builtin_skill = builtin_dir.join(name).join("SKILL.md");
            if builtin_skill.exists() {
                return std::fs::read_to_string(&builtin_skill).ok();
            }
        }

        None
    }

    /// Load specific skills for inclusion in agent context.
    pub fn load_skills_for_context(&self, skill_names: &[String]) -> String {
        let mut parts = Vec::new();
        for name in skill_names {
            if let Some(content) = self.load_skill(name) {
                let body = markdown::strip_frontmatter(&content);
                parts.push(format!("### Skill: {name}\n\n{body}"));
            }
        }
        parts.join("\n\n---\n\n")
    }

    /// Build a summary of all skills.
    pub fn build_skills_summary(&self) -> String {
        let all_skills = self.list_skills(false);
        if all_skills.is_empty() {
            return String::new();
        }

        let mut lines = vec!["<skills>".to_string()];
        for s in &all_skills {
            let name = escape_xml(&s.name);
            let desc = escape_xml(&self.get_skill_description(&s.name));
            let meta = self.get_skill_nanobot_meta(&s.name);
            let available = check_requirements(&meta);

            lines.push(format!(
                "  <skill available=\"{}\">",
                if available { "true" } else { "false" }
            ));
            lines.push(format!("    <name>{name}</name>"));
            lines.push(format!("    <description>{desc}</description>"));
            lines.push(format!("    <location>{}</location>", s.path.display()));

            if !available {
                let missing = get_missing_requirements(&meta);
                if !missing.is_empty() {
                    lines.push(format!("    <requires>{}</requires>", escape_xml(&missing)));
                }
            }

            lines.push("  </skill>".to_string());
        }
        lines.push("</skills>".to_string());
        lines.join("\n")
    }

    /// Get skills marked as always=true that meet requirements.
    pub fn get_always_skills(&self) -> Vec<String> {
        let mut result = Vec::new();
        for s in self.list_skills(true) {
            let frontmatter = self.get_skill_metadata(&s.name);
            let nanobot_meta = self.get_skill_nanobot_meta(&s.name);
            if nanobot_meta.contains_key("always")
                || frontmatter
                    .as_ref()
                    .and_then(|m| m.get("always"))
                    .map(|v| v == "true")
                    .unwrap_or(false)
            {
                result.push(s.name);
            }
        }
        result
    }

    /// Get metadata from a skill's frontmatter.
    pub fn get_skill_metadata(&self, name: &str) -> Option<HashMap<String, String>> {
        let content = self.load_skill(name)?;
        let (meta, _) = markdown::parse_frontmatter(&content);
        if meta.is_empty() {
            None
        } else {
            Some(meta)
        }
    }

    fn get_skill_description(&self, name: &str) -> String {
        self.get_skill_metadata(name)
            .and_then(|m| m.get("description").cloned())
            .unwrap_or_else(|| name.to_string())
    }

    fn get_skill_nanobot_meta(&self, name: &str) -> HashMap<String, serde_json::Value> {
        if let Some(meta) = self.get_skill_metadata(name) {
            if let Some(raw) = meta.get("metadata") {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(nanobot) = data.get("nanobot") {
                        if let Ok(m) = serde_json::from_value(nanobot.clone()) {
                            return m;
                        }
                    }
                }
            }
        }
        HashMap::new()
    }
}

fn check_requirements(meta: &HashMap<String, serde_json::Value>) -> bool {
    if let Some(requires) = meta.get("requires") {
        if let Some(bins) = requires.get("bins").and_then(|v| v.as_array()) {
            for bin in bins {
                if let Some(name) = bin.as_str() {
                    if which::which(name).is_err() {
                        return false;
                    }
                }
            }
        }
        if let Some(envs) = requires.get("env").and_then(|v| v.as_array()) {
            for env in envs {
                if let Some(name) = env.as_str() {
                    if std::env::var(name).is_err() {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn get_missing_requirements(meta: &HashMap<String, serde_json::Value>) -> String {
    let mut missing = Vec::new();
    if let Some(requires) = meta.get("requires") {
        if let Some(bins) = requires.get("bins").and_then(|v| v.as_array()) {
            for bin in bins {
                if let Some(name) = bin.as_str() {
                    if which::which(name).is_err() {
                        missing.push(format!("CLI: {name}"));
                    }
                }
            }
        }
        if let Some(envs) = requires.get("env").and_then(|v| v.as_array()) {
            for env in envs {
                if let Some(name) = env.as_str() {
                    if std::env::var(name).is_err() {
                        missing.push(format!("ENV: {name}"));
                    }
                }
            }
        }
    }
    missing.join(", ")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
