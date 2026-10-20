import { LauncherColors } from '../theme/LauncherTheme';

@Component
export struct LauncherSectionCard {
  @Prop title: string = '';
  @BuilderParam content: () => void = this.emptyContent;

  @Builder
  private emptyContent() {}

  build() {
    Column({ space: 16 }) {
      Text(this.title).fontSize(20).fontWeight(FontWeight.Medium)
        .fontColor(LauncherColors.text).width('100%')
      this.content()
    }
    .width('100%')
    .padding(20)
    .alignItems(HorizontalAlign.Start)
    .backgroundColor(LauncherColors.card)
    .borderRadius(24)
    .border({ width: 1, color: LauncherColors.border })
  }
}
