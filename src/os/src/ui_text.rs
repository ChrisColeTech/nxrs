//! The host prompts' texts in the game's language (the `language` option, 0..14 as the console's language codes):
//! the controller prompt, the Home quit prompt and the profile picker. nxnative lets its front end pass them
//! (`DM_CTRL_PROMPT_TEXT`, `DM_HOME_QUIT_TEXT`); here the engine has its own table, and a front end may still override
//! any line with `NXRS_CTRL_PROMPT_TEXT`, `NXRS_HOME_QUIT_TEXT`, `NXRS_USER_PROMPT_TEXT` (the lines of a prompt, one per
//! line: its title, then its hint; an empty or missing line keeps the table's).
//!
//! `{min}` and `{joined}` are replaced by the numbers.

/// The texts of one language: controller prompt title / hint, Home quit title / hint, profile picker title / hint.
struct Lang([&'static str; 6]);

const EN: Lang = Lang([
    "{min} players wanted",
    "Press a button on each controller to join.   Players: {joined} of {min}   (A / Return: done, B / Esc: cancel)",
    "Quit the game?",
    "A / Return: quit.   B / Esc / Home: keep playing.",
    "Who is playing?",
    "Up / Down: choose.   A / Return: select.   B / Esc: cancel",
]);
const JA: Lang = Lang([
    "{min}人のプレイヤーが必要です",
    "参加するコントローラーのボタンを押してください。   プレイヤー: {joined} / {min}   (A / Enter: 決定、B / Esc: キャンセル)",
    "ゲームを終了しますか？",
    "A / Enter: 終了   B / Esc / HOME: プレイを続ける",
    "だれがあそびますか？",
    "上 / 下: 選ぶ   A / Enter: 決定   B / Esc: キャンセル",
]);
const FR: Lang = Lang([
    "{min} joueurs requis",
    "Appuyez sur un bouton de chaque manette pour rejoindre.   Joueurs : {joined} sur {min}   (A / Entrée : terminé, B / Échap : annuler)",
    "Quitter le jeu ?",
    "A / Entrée : quitter.   B / Échap / HOME : continuer à jouer.",
    "Qui joue ?",
    "Haut / Bas : choisir.   A / Entrée : valider.   B / Échap : annuler",
]);
const DE: Lang = Lang([
    "{min} Spieler benötigt",
    "Drücke auf jedem Controller eine Taste, um beizutreten.   Spieler: {joined} von {min}   (A / Eingabe: fertig, B / Esc: abbrechen)",
    "Spiel beenden?",
    "A / Eingabe: beenden.   B / Esc / HOME: weiterspielen.",
    "Wer spielt?",
    "Oben / Unten: wählen.   A / Eingabe: auswählen.   B / Esc: abbrechen",
]);
const IT: Lang = Lang([
    "Servono {min} giocatori",
    "Premi un pulsante su ogni controller per partecipare.   Giocatori: {joined} su {min}   (A / Invio: fatto, B / Esc: annulla)",
    "Uscire dal gioco?",
    "A / Invio: esci.   B / Esc / HOME: continua a giocare.",
    "Chi gioca?",
    "Su / Giù: scegli.   A / Invio: conferma.   B / Esc: annulla",
]);
const ES: Lang = Lang([
    "Se necesitan {min} jugadores",
    "Pulsa un botón en cada mando para unirte.   Jugadores: {joined} de {min}   (A / Intro: listo, B / Esc: cancelar)",
    "¿Salir del juego?",
    "A / Intro: salir.   B / Esc / HOME: seguir jugando.",
    "¿Quién juega?",
    "Arriba / Abajo: elegir.   A / Intro: aceptar.   B / Esc: cancelar",
]);
const ES_419: Lang = Lang([
    "Se necesitan {min} jugadores",
    "Presiona un botón en cada control para unirte.   Jugadores: {joined} de {min}   (A / Enter: listo, B / Esc: cancelar)",
    "¿Salir del juego?",
    "A / Enter: salir.   B / Esc / HOME: seguir jugando.",
    "¿Quién juega?",
    "Arriba / Abajo: elegir.   A / Enter: aceptar.   B / Esc: cancelar",
]);
const ZH_HANS: Lang = Lang([
    "需要{min}名玩家",
    "请按下每个手柄上的按钮加入。   玩家：{joined} / {min}   （A / 回车：完成，B / Esc：取消）",
    "要退出游戏吗？",
    "A / 回车：退出。   B / Esc / HOME：继续游戏。",
    "谁来玩？",
    "上 / 下：选择。   A / 回车：确定。   B / Esc：取消",
]);
const ZH_HANT: Lang = Lang([
    "需要{min}名玩家",
    "請按下每個控制器上的按鈕加入。   玩家：{joined} / {min}   （A / Enter：完成，B / Esc：取消）",
    "要結束遊戲嗎？",
    "A / Enter：結束。   B / Esc / HOME：繼續遊玩。",
    "誰要玩？",
    "上 / 下：選擇。   A / Enter：確定。   B / Esc：取消",
]);
const KO: Lang = Lang([
    "플레이어 {min}명이 필요합니다",
    "참가하려면 각 컨트롤러의 버튼을 누르세요.   플레이어: {joined} / {min}   (A / Enter: 완료, B / Esc: 취소)",
    "게임을 종료하시겠습니까?",
    "A / Enter: 종료   B / Esc / HOME: 계속 플레이",
    "누가 플레이하나요?",
    "위 / 아래: 선택   A / Enter: 결정   B / Esc: 취소",
]);
const NL: Lang = Lang([
    "{min} spelers nodig",
    "Druk op een knop van elke controller om mee te doen.   Spelers: {joined} van {min}   (A / Enter: klaar, B / Esc: annuleren)",
    "Het spel afsluiten?",
    "A / Enter: afsluiten.   B / Esc / HOME: verder spelen.",
    "Wie speelt er?",
    "Omhoog / Omlaag: kiezen.   A / Enter: bevestigen.   B / Esc: annuleren",
]);
const PT: Lang = Lang([
    "São necessários {min} jogadores",
    "Prima um botão em cada comando para entrar.   Jogadores: {joined} de {min}   (A / Enter: concluir, B / Esc: cancelar)",
    "Sair do jogo?",
    "A / Enter: sair.   B / Esc / HOME: continuar a jogar.",
    "Quem vai jogar?",
    "Cima / Baixo: escolher.   A / Enter: confirmar.   B / Esc: cancelar",
]);
const RU: Lang = Lang([
    "Нужно игроков: {min}",
    "Нажмите кнопку на каждом контроллере, чтобы присоединиться.   Игроки: {joined} из {min}   (A / Enter: готово, B / Esc: отмена)",
    "Выйти из игры?",
    "A / Enter: выйти.   B / Esc / HOME: продолжить игру.",
    "Кто играет?",
    "Вверх / Вниз: выбор.   A / Enter: подтвердить.   B / Esc: отмена",
]);

/// The table of a `language` value (the console's order; English for anything else).
fn table(language: i64) -> &'static Lang {
    match language {
        0 => &JA,
        2 | 13 => &FR,
        3 => &DE,
        4 => &IT,
        5 => &ES,
        6 => &ZH_HANS,
        7 => &KO,
        8 => &NL,
        9 => &PT,
        10 => &RU,
        11 => &ZH_HANT,
        14 => &ES_419,
        _ => &EN,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    Controllers,
    HomeQuit,
    UserPicker,
}

/// (title, hint) of a prompt in `language`, with a front end's override lines and the numbers put in.
pub fn texts_in(
    p: Prompt,
    language: i64,
    over: Option<&str>,
    min: i32,
    joined: i32,
) -> (String, String) {
    let t = table(language);
    let i = match p {
        Prompt::Controllers => 0,
        Prompt::HomeQuit => 2,
        Prompt::UserPicker => 4,
    };
    let lines: Vec<&str> = over.map(|o| o.lines().collect()).unwrap_or_default();
    let pick = |k: usize| {
        lines
            .get(k)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(t.0[i + k])
    };
    let fill = |s: &str| {
        s.replace("{min}", &min.to_string())
            .replace("{joined}", &joined.to_string())
    };
    (fill(pick(0)), fill(pick(1)))
}

/// (title, hint) of a prompt in the game's language (`language`, read live).
pub fn texts(p: Prompt, min: i32, joined: i32) -> (String, String) {
    let env = match p {
        Prompt::Controllers => "NXRS_CTRL_PROMPT_TEXT",
        Prompt::HomeQuit => "NXRS_HOME_QUIT_TEXT",
        Prompt::UserPicker => "NXRS_USER_PROMPT_TEXT",
    };
    texts_in(
        p,
        nxconfig::int("language"),
        std::env::var(env).ok().as_deref(),
        min,
        joined,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_and_overrides() {
        let (t, h) = texts_in(Prompt::Controllers, 1, None, 2, 1);
        assert_eq!(t, "2 players wanted");
        assert!(h.contains("Players: 1 of 2"));
        assert_eq!(
            texts_in(Prompt::HomeQuit, 0, None, 0, 0).0,
            "ゲームを終了しますか？"
        );
        assert_eq!(
            texts_in(Prompt::HomeQuit, 3, None, 0, 0).0,
            "Spiel beenden?"
        );
        assert_eq!(
            texts_in(Prompt::UserPicker, 13, None, 0, 0).0,
            "Qui joue ?",
            "French (Canada)"
        );
        assert_eq!(
            texts_in(Prompt::UserPicker, 12, None, 0, 0).0,
            "Who is playing?",
            "English (UK)"
        );
        assert_eq!(
            texts_in(Prompt::UserPicker, 99, None, 0, 0).0,
            "Who is playing?",
            "unknown: English"
        );
        // a front end's line replaces the table's; an empty line keeps it
        let (t, h) = texts_in(
            Prompt::Controllers,
            2,
            Some("\nAppuie {joined}/{min}"),
            3,
            2,
        );
        assert_eq!(t, "3 joueurs requis");
        assert_eq!(h, "Appuie 2/3");
        // every language has all six texts
        for l in 0..15 {
            assert!(table(l).0.iter().all(|s| !s.is_empty()));
            assert!(table(l).0[1].contains("{joined}") && table(l).0[1].contains("{min}"));
        }
    }
}
