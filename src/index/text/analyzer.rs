//! 保留原始UTF-8范围的ASCII词项与CJK双字分析器。

pub const TEXT_ANALYZER_ID: &str = "tdb-text-ascii-cjk2-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnalyzedToken {
    pub term: String,
    pub position: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

struct Scanner<F> {
    emit: F,
    position: usize,
    word: String,
    word_start: usize,
    word_end: usize,
    previous_cjk: Option<(char, usize, usize)>,
    cjk_has_pair: bool,
}

impl<F> Scanner<F> {
    fn emit<E>(&mut self, term: String, start: usize, end: usize) -> Result<(), E>
    where
        F: FnMut(AnalyzedToken) -> Result<(), E>,
    {
        let token = AnalyzedToken {
            term,
            position: self.position,
            start_byte: start,
            end_byte: end,
        };
        // 每个词项至少消耗一个输入字符，序号受输入字符串长度约束。
        self.position = self.position.saturating_add(1);
        (self.emit)(token)
    }

    fn flush_word<E>(&mut self) -> Result<(), E>
    where
        F: FnMut(AnalyzedToken) -> Result<(), E>,
    {
        if self.word.is_empty() {
            return Ok(());
        }
        let word = std::mem::take(&mut self.word);
        self.emit(word, self.word_start, self.word_end)
    }

    fn flush_cjk<E>(&mut self) -> Result<(), E>
    where
        F: FnMut(AnalyzedToken) -> Result<(), E>,
    {
        if let Some((ch, start, end)) = self.previous_cjk.take()
            && !self.cjk_has_pair
        {
            self.emit(ch.to_string(), start, end)?;
        }
        self.cjk_has_pair = false;
        Ok(())
    }

    fn push<E>(&mut self, ch: char, start: usize, end: usize) -> Result<(), E>
    where
        F: FnMut(AnalyzedToken) -> Result<(), E>,
    {
        if ch.is_ascii_alphanumeric() {
            self.flush_cjk()?;
            if self.word.is_empty() {
                self.word_start = start;
            }
            self.word.push(ch);
            self.word_end = end;
        } else if ('\u{3400}'..='\u{9fff}').contains(&ch)
            || ('\u{3040}'..='\u{30ff}').contains(&ch)
            || ('\u{ac00}'..='\u{d7af}').contains(&ch)
        {
            self.flush_word()?;
            if let Some((previous, first, _)) = self.previous_cjk {
                let mut pair = String::with_capacity(8);
                pair.push(previous);
                pair.push(ch);
                self.emit(pair, first, end)?;
                self.cjk_has_pair = true;
            }
            self.previous_cjk = Some((ch, start, end));
        } else {
            self.flush_word()?;
            self.flush_cjk()?;
        }
        Ok(())
    }
}

/// 回调在词项发布前准入，原始字符边界同时覆盖大小写展开产生的字符。
pub(crate) fn visit_tokens<E>(
    text: &str,
    emit: impl FnMut(AnalyzedToken) -> Result<(), E>,
    mut check: impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    let mut scanner = Scanner {
        emit,
        position: 0,
        word: String::new(),
        word_start: 0,
        word_end: 0,
        previous_cjk: None,
        cjk_has_pair: false,
    };
    for (index, (start, ch)) in text.char_indices().enumerate() {
        if index % 1024 == 0 {
            check()?;
        }
        // char_indices保证该字符的全部UTF-8字节位于输入字符串之内。
        let end = start.saturating_add(ch.len_utf8());
        for lowered in ch.to_lowercase() {
            scanner.push(lowered, start, end)?;
        }
    }
    scanner.flush_word()?;
    scanner.flush_cjk()?;
    check()
}

pub(super) fn tokenize(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let result = visit_tokens(
        text,
        |token| {
            terms.push(token.term);
            Ok::<(), std::convert::Infallible>(())
        },
        || Ok(()),
    );
    match result {
        Ok(()) => terms,
        Err(never) => match never {},
    }
}
