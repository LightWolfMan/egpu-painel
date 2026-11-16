//! Idioma da interface: português (padrão do projeto) ou inglês.
//!
//! Sem tabela de chaves: cada texto fica no próprio lugar com as duas versões lado a lado,
//! `t("Português", "English")`, e `tf!` faz o mesmo para `format!`. Quem lê o código vê as duas
//! frases juntas e não precisa caçar a tradução em outro arquivo.
//!
//! Ordem de escolha: `EGPU_LANG=pt|en` (teste) > `config.json` (`lang`: 1 = pt, 2 = en) >
//! idioma da interface do Windows (pt-* → português, qualquer outro → inglês).

use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

/// Inglês ativo?
#[inline]
pub fn en() -> bool {
    EN.load(Ordering::Relaxed)
}

/// Escolhe o texto do idioma ativo.
#[inline]
pub fn t(pt: &'static str, en: &'static str) -> &'static str {
    if self::en() { en } else { pt }
}

/// `format!` bilíngue: `tf!("há {n} s", "{n} s ago")`. Os dois formatos recebem os mesmos argumentos.
#[macro_export]
macro_rules! tf {
    ($pt:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if $crate::i18n::en() { format!($en $(, $a)*) } else { format!($pt $(, $a)*) }
    };
}

/// Idioma da interface do Windows é português?
fn windows_is_pt() -> bool {
    // LANGID: os 10 bits baixos são o idioma primário; 0x16 = LANG_PORTUGUESE (pt-BR e pt-PT).
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    id & 0x3FF == 0x16
}

/// Resolve o idioma a partir do valor salvo (0 = automático, 1 = pt, 2 = en).
pub fn apply(lang: u8) {
    let en = match std::env::var("EGPU_LANG").ok().as_deref() {
        Some("pt") => false,
        Some("en") => true,
        _ => match lang {
            1 => false,
            2 => true,
            _ => !windows_is_pt(),
        },
    };
    EN.store(en, Ordering::Relaxed);
}

/// Aplica o idioma do `config.json` (chamar no início de cada processo).
pub fn init() {
    apply(crate::config::get().lang);
}

/// Separador decimal do idioma: vírgula em português, ponto em inglês.
pub fn dec_sep(s: String) -> String {
    if en() { s } else { s.replace('.', ",") }
}
