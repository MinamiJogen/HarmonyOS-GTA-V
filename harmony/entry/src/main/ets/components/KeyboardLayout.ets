// Standard US 104-key layout; codes stay distinct across physical key groups.
export interface KeyboardKey { code: string; label: string; width: number; }
export interface KeyboardRow { id: string; keys: KeyboardKey[]; }
export interface KeyboardPartition { name: string; rows: KeyboardRow[]; }
export const KEYBOARD_PARTITIONS: KeyboardPartition[] = [
  {
    "name": "主键区",
    "rows": [
      {
        "id": "0",
        "keys": [
          {
            "code": "Escape",
            "label": "Esc",
            "width": 48
          },
          {
            "code": "F1",
            "label": "F1",
            "width": 48
          },
          {
            "code": "F2",
            "label": "F2",
            "width": 48
          },
          {
            "code": "F3",
            "label": "F3",
            "width": 48
          },
          {
            "code": "F4",
            "label": "F4",
            "width": 48
          },
          {
            "code": "F5",
            "label": "F5",
            "width": 48
          },
          {
            "code": "F6",
            "label": "F6",
            "width": 48
          },
          {
            "code": "F7",
            "label": "F7",
            "width": 48
          },
          {
            "code": "F8",
            "label": "F8",
            "width": 48
          },
          {
            "code": "F9",
            "label": "F9",
            "width": 48
          },
          {
            "code": "F10",
            "label": "F10",
            "width": 48
          },
          {
            "code": "F11",
            "label": "F11",
            "width": 48
          },
          {
            "code": "F12",
            "label": "F12",
            "width": 48
          }
        ]
      },
      {
        "id": "1",
        "keys": [
          {
            "code": "Backquote",
            "label": "` ~",
            "width": 48
          },
          {
            "code": "Digit1",
            "label": "1",
            "width": 48
          },
          {
            "code": "Digit2",
            "label": "2",
            "width": 48
          },
          {
            "code": "Digit3",
            "label": "3",
            "width": 48
          },
          {
            "code": "Digit4",
            "label": "4",
            "width": 48
          },
          {
            "code": "Digit5",
            "label": "5",
            "width": 48
          },
          {
            "code": "Digit6",
            "label": "6",
            "width": 48
          },
          {
            "code": "Digit7",
            "label": "7",
            "width": 48
          },
          {
            "code": "Digit8",
            "label": "8",
            "width": 48
          },
          {
            "code": "Digit9",
            "label": "9",
            "width": 48
          },
          {
            "code": "Digit0",
            "label": "0",
            "width": 48
          },
          {
            "code": "Minus",
            "label": "- _",
            "width": 48
          },
          {
            "code": "Equal",
            "label": "= +",
            "width": 48
          },
          {
            "code": "Backspace",
            "label": "⌫",
            "width": 66
          }
        ]
      },
      {
        "id": "2",
        "keys": [
          {
            "code": "Tab",
            "label": "Tab",
            "width": 66
          },
          {
            "code": "KeyQ",
            "label": "Q",
            "width": 48
          },
          {
            "code": "KeyW",
            "label": "W",
            "width": 48
          },
          {
            "code": "KeyE",
            "label": "E",
            "width": 48
          },
          {
            "code": "KeyR",
            "label": "R",
            "width": 48
          },
          {
            "code": "KeyT",
            "label": "T",
            "width": 48
          },
          {
            "code": "KeyY",
            "label": "Y",
            "width": 48
          },
          {
            "code": "KeyU",
            "label": "U",
            "width": 48
          },
          {
            "code": "KeyI",
            "label": "I",
            "width": 48
          },
          {
            "code": "KeyO",
            "label": "O",
            "width": 48
          },
          {
            "code": "KeyP",
            "label": "P",
            "width": 48
          },
          {
            "code": "BracketLeft",
            "label": "[ {",
            "width": 48
          },
          {
            "code": "BracketRight",
            "label": "] }",
            "width": 48
          },
          {
            "code": "Backslash",
            "label": "\\ |",
            "width": 48
          }
        ]
      },
      {
        "id": "3",
        "keys": [
          {
            "code": "CapsLock",
            "label": "Caps",
            "width": 66
          },
          {
            "code": "KeyA",
            "label": "A",
            "width": 48
          },
          {
            "code": "KeyS",
            "label": "S",
            "width": 48
          },
          {
            "code": "KeyD",
            "label": "D",
            "width": 48
          },
          {
            "code": "KeyF",
            "label": "F",
            "width": 48
          },
          {
            "code": "KeyG",
            "label": "G",
            "width": 48
          },
          {
            "code": "KeyH",
            "label": "H",
            "width": 48
          },
          {
            "code": "KeyJ",
            "label": "J",
            "width": 48
          },
          {
            "code": "KeyK",
            "label": "K",
            "width": 48
          },
          {
            "code": "KeyL",
            "label": "L",
            "width": 48
          },
          {
            "code": "Semicolon",
            "label": "; :",
            "width": 48
          },
          {
            "code": "Quote",
            "label": "' \"",
            "width": 48
          },
          {
            "code": "Enter",
            "label": "Enter",
            "width": 66
          }
        ]
      },
      {
        "id": "4",
        "keys": [
          {
            "code": "ShiftLeft",
            "label": "Shift L",
            "width": 66
          },
          {
            "code": "KeyZ",
            "label": "Z",
            "width": 48
          },
          {
            "code": "KeyX",
            "label": "X",
            "width": 48
          },
          {
            "code": "KeyC",
            "label": "C",
            "width": 48
          },
          {
            "code": "KeyV",
            "label": "V",
            "width": 48
          },
          {
            "code": "KeyB",
            "label": "B",
            "width": 48
          },
          {
            "code": "KeyN",
            "label": "N",
            "width": 48
          },
          {
            "code": "KeyM",
            "label": "M",
            "width": 48
          },
          {
            "code": "Comma",
            "label": ", <",
            "width": 48
          },
          {
            "code": "Period",
            "label": ". >",
            "width": 48
          },
          {
            "code": "Slash",
            "label": "/ ?",
            "width": 48
          },
          {
            "code": "ShiftRight",
            "label": "Shift R",
            "width": 66
          }
        ]
      },
      {
        "id": "5",
        "keys": [
          {
            "code": "ControlLeft",
            "label": "Ctrl L",
            "width": 48
          },
          {
            "code": "MetaLeft",
            "label": "Win L",
            "width": 48
          },
          {
            "code": "AltLeft",
            "label": "Alt L",
            "width": 48
          },
          {
            "code": "Space",
            "label": "Space",
            "width": 132
          },
          {
            "code": "AltRight",
            "label": "Alt R",
            "width": 48
          },
          {
            "code": "MetaRight",
            "label": "Win R",
            "width": 48
          },
          {
            "code": "ContextMenu",
            "label": "Menu",
            "width": 48
          },
          {
            "code": "ControlRight",
            "label": "Ctrl R",
            "width": 48
          }
        ]
      }
    ]
  },
  {
    "name": "导航区",
    "rows": [
      {
        "id": "0",
        "keys": [
          {
            "code": "PrintScreen",
            "label": "PrtSc",
            "width": 48
          },
          {
            "code": "ScrollLock",
            "label": "ScrLk",
            "width": 48
          },
          {
            "code": "Pause",
            "label": "Pause",
            "width": 48
          }
        ]
      },
      {
        "id": "1",
        "keys": [
          {
            "code": "Insert",
            "label": "Insert",
            "width": 48
          },
          {
            "code": "Home",
            "label": "Home",
            "width": 48
          },
          {
            "code": "PageUp",
            "label": "PgUp",
            "width": 48
          }
        ]
      },
      {
        "id": "2",
        "keys": [
          {
            "code": "Delete",
            "label": "Delete",
            "width": 48
          },
          {
            "code": "End",
            "label": "End",
            "width": 48
          },
          {
            "code": "PageDown",
            "label": "PgDn",
            "width": 48
          }
        ]
      },
      {
        "id": "3",
        "keys": [
          {
            "code": "ArrowUp",
            "label": "↑",
            "width": 48
          }
        ]
      },
      {
        "id": "4",
        "keys": [
          {
            "code": "ArrowLeft",
            "label": "←",
            "width": 48
          },
          {
            "code": "ArrowDown",
            "label": "↓",
            "width": 48
          },
          {
            "code": "ArrowRight",
            "label": "→",
            "width": 48
          }
        ]
      }
    ]
  },
  {
    "name": "数字小键盘",
    "rows": [
      {
        "id": "0",
        "keys": [
          {
            "code": "NumLock",
            "label": "Num",
            "width": 48
          },
          {
            "code": "NumpadDivide",
            "label": "÷",
            "width": 48
          },
          {
            "code": "NumpadMultiply",
            "label": "×",
            "width": 48
          },
          {
            "code": "NumpadSubtract",
            "label": "−",
            "width": 48
          }
        ]
      },
      {
        "id": "1",
        "keys": [
          {
            "code": "Numpad7",
            "label": "7",
            "width": 48
          },
          {
            "code": "Numpad8",
            "label": "8",
            "width": 48
          },
          {
            "code": "Numpad9",
            "label": "9",
            "width": 48
          },
          {
            "code": "NumpadAdd",
            "label": "+",
            "width": 48
          }
        ]
      },
      {
        "id": "2",
        "keys": [
          {
            "code": "Numpad4",
            "label": "4",
            "width": 48
          },
          {
            "code": "Numpad5",
            "label": "5",
            "width": 48
          },
          {
            "code": "Numpad6",
            "label": "6",
            "width": 48
          }
        ]
      },
      {
        "id": "3",
        "keys": [
          {
            "code": "Numpad1",
            "label": "1",
            "width": 48
          },
          {
            "code": "Numpad2",
            "label": "2",
            "width": 48
          },
          {
            "code": "Numpad3",
            "label": "3",
            "width": 48
          },
          {
            "code": "NumpadEnter",
            "label": "Enter ⌨",
            "width": 48
          }
        ]
      },
      {
        "id": "4",
        "keys": [
          {
            "code": "Numpad0",
            "label": "0",
            "width": 48
          },
          {
            "code": "NumpadDecimal",
            "label": ".",
            "width": 48
          }
        ]
      }
    ]
  }
];
