pub enum Mode {
    Chat,   // 通常チャットモード
    Code,   // コーディング支援モード
    Pair,   // ペアプログラミングモード
}

impl Mode {
    pub fn get_prompt(&self) -> &str {
        match self {
            Mode::Chat => ">> ",
            Mode::Code => "? ",
            Mode::Pair => "! ",
        }
    }

    pub fn get_features(&self) -> Vec<&str> {
        match self {
            Mode::Chat => vec![
                "自然な会話",
                "質問応答",
                "タスク管理",
                "情報検索",
            ],
            Mode::Code => vec![
                "コード補完",
                "シンタックスハイライト",
                "エラー検出",
                "ドキュメント生成",
                "コードレビュー提案",
                "リファクタリング提案",
            ],
            Mode::Pair => vec![
                "リアルタイムコードレビュー",
                "コード説明",
                "改善提案",
                "ベストプラクティス共有",
                "テストケース提案",
            ],
        }
    }

    pub fn get_behavior(&self) -> &str {
        match self {
            Mode::Chat => "通常の会話モード：自然な対話でタスクをサポート",
            Mode::Code => "コーディング支援モード：コード分析、補完、最適化を提供",
            Mode::Pair => "ペアプログラミングモード：レビューと改善提案をリアルタイムで実施",
        }
    }
}