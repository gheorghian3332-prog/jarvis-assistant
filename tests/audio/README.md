# Тестовые WAV для голосового фронта

Файлы 16 кГц, моно, PCM16. Сгенерированы `scripts/make-test-wavs.ps1`: фразы синтезирует локальный Piper
(`ru_RU-dmitri-medium`), затем скрипт ресемплирует их и добавляет по 0.5 с тишины до и после; тишина и шум
создаются самим скриптом (фиксированное зерно, результат повторяем). Микрофон не используется.

| Файл | Содержимое | Ожидание |
|---|---|---|
| `wake_open_notepad.wav` | «Лёха, открой блокнот» | `WakeDetected` + `Utterance` (source=wake) |
| `wake_only.wav` | «Лёха» | `WakeDetected`, затем `LISTENING` без команды (Utterance нет) |
| `alexey_louder.wav` | «Алексей, сделай громче» | `WakeDetected` (алексей) + `Utterance` |
| `wake_time.wav` | «Лёха, скажи, сколько сейчас времени» | `WakeDetected` + `Utterance`; в e2e: STT → `info.time` → речь |
| `neg_hello.wav` | «привет, как дела» | нет событий |
| `neg_name_mid_phrase.wav` | «леха» в середине длинной фразы | нет событий (правило позиции) |
| `silence_5s.wav` | 5 с цифровой тишины | нет событий; `no_signal=true` |
| `noise_5s.wav` | 5 с белого шума | нет событий |

Прогон: `target\debug\jarvisd.exe --voice-wav tests\audio\<файл>.wav` или `cargo test -p jarvis-voice --test wav`
(нужны модели: `scripts/fetch-models.ps1`).
