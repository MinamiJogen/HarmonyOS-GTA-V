import { AdvancedDialogV2Button, CustomContentDialogV2 } from '@kit.ArkUI';
import { launcherTheme } from '../theme/LauncherTheme';

export interface GameSaveEntry {
  id: string;
  title: string;
  file: string;
  size: number;
  mtime: number;
  mode: number;
  backup: boolean;
}

// Keep the title, action area, scrolling limits and surface under the system dialog.
@ComponentV2
export struct SavePickerDialog {
  @Param saves: GameSaveEntry[] = [];
  @Event onSelect: (id: string) => void = (_id: string): void => {};
  @Event onClose: () => void = (): void => {};

  private fileName(item: GameSaveEntry): string {
    return item.file.slice(item.file.lastIndexOf('/') + 1);
  }

  private sizeLabel(bytes: number): string {
    if (bytes >= 1024 * 1024) { return `${(bytes / (1024 * 1024)).toFixed(1)} MB`; }
    if (bytes >= 1024) { return `${(bytes / 1024).toFixed(1)} KB`; }
    return `${bytes} B`;
  }

  private description(item: GameSaveEntry): string {
    const mode: string = item.mode === 2 ? 'Demo' : '剧情';
    const date: string = item.mtime > 0 ? new Date(item.mtime * 1000).toLocaleString() : '时间未知';
    return `${this.fileName(item)} · ${mode} · ${date} · ${this.sizeLabel(item.size)}`;
  }

  @Builder
  private saveItems(): void {
    Scroll() {
      Column({ space: 4 }) {
        ForEach(this.saves, (item: GameSaveEntry) => {
          Button({ type: ButtonType.Normal }) {
            Column({ space: 6 }) {
              Text(item.title || this.fileName(item))
                .fontSize(16)
                .fontWeight(FontWeight.Medium)
                .fontColor($r('sys.color.font_primary'))
                .textAlign(TextAlign.Start)
                .width('100%')
              Text(this.description(item))
                .fontSize(12)
                .fontColor($r('sys.color.font_secondary'))
                .textAlign(TextAlign.Start)
                .width('100%')
            }
            .alignItems(HorizontalAlign.Start)
            .padding({ top: 12, bottom: 12, left: 12, right: 12 })
            .width('100%')
          }
          .width('100%')
          .constraintSize({ minHeight: 64 })
          .borderRadius(12)
          .buttonStyle(ButtonStyleMode.TEXTUAL)
          .accessibilityGroup(true)
          .accessibilityText(`${item.title || this.fileName(item)}，${this.description(item)}`)
          .accessibilityDescription('加载此存档')
          .onClick(() => { this.onSelect(item.id); })
        }, (item: GameSaveEntry) => item.id)
      }
      .width('100%')
    }
    .width('100%')
    .constraintSize({ maxHeight: 360 })
    .scrollBar(BarState.Auto)
  }

  build() {
    WithTheme({ theme: launcherTheme, colorMode: ThemeColorMode.LIGHT }) {
      CustomContentDialogV2({
        primaryTitle: '加载剧情存档',
        secondaryTitle: '选择一个存档继续游戏',
        contentBuilder: () => { this.saveItems(); },
        buttons: [
          new AdvancedDialogV2Button({
            content: '取消',
            action: () => { this.onClose(); }
          })
        ]
      })
    }
  }
}
