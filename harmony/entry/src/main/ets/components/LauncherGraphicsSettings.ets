import {
  ColorMetrics, LengthMetrics, SegmentButtonV2Items, TabSegmentButtonV2, uiMaterial
} from '@kit.ArkUI';
import { LauncherColors } from '../theme/LauncherTheme';

@Component
export struct LauncherGraphicsSettings {
  @Prop preset: string = 'medium';
  @Prop superResolution: boolean = true;
  @Prop frameGeneration: boolean = true;
  @Prop settingsEnabled: boolean = true;

  onPresetChange: (preset: string) => boolean = (_preset: string): boolean => false;
  onSuperResolutionChange: (enabled: boolean) => boolean = (_enabled: boolean): boolean => false;
  onFrameGenerationChange: (enabled: boolean) => boolean = (_enabled: boolean): boolean => false;

  @State private presetControlEpoch: number = 0;
  @State private superResolutionControlEpoch: number = 0;
  @State private frameGenerationControlEpoch: number = 0;

  private readonly qualityItems: SegmentButtonV2Items = new SegmentButtonV2Items([
    { text: '低', accessibilityText: '低画质' },
    { text: '中', accessibilityText: '中画质' },
    { text: '高', accessibilityText: '高画质' }
  ]);
  private readonly controlMaterial: uiMaterial.Material = new uiMaterial.ImmersiveMaterial({
    style: uiMaterial.ImmersiveStyle.ULTRA_THIN,
    interactive: true,
    lightEffect: {}
  });

  private selectedIndex(): number {
    return this.preset === 'low' ? 0 : (this.preset === 'high' ? 2 : 1);
  }

  private requestPreset(index: number): void {
    if (index !== 0 && index !== 1 && index !== 2) { return; }
    const next: string = index === 0 ? 'low' : (index === 2 ? 'high' : 'medium');
    if (!this.settingsEnabled || next === this.preset) { return; }
    // The parent commits the persisted value. Recreate only a rejected native selection,
    // whose internal interaction state could otherwise outlive an unchanged @Prop.
    if (!this.onPresetChange(next)) { this.presetControlEpoch++; }
  }

  private requestSuperResolution(next: boolean): void {
    if (!this.settingsEnabled || next === this.superResolution) { return; }
    if (!this.onSuperResolutionChange(next)) { this.superResolutionControlEpoch++; }
  }

  private requestFrameGeneration(next: boolean): void {
    if (!this.settingsEnabled || next === this.frameGeneration) { return; }
    if (!this.onFrameGenerationChange(next)) { this.frameGenerationControlEpoch++; }
  }

  build() {
    Flex({
      direction: FlexDirection.Row,
      wrap: FlexWrap.Wrap,
      alignItems: ItemAlign.Center,
      justifyContent: FlexAlign.SpaceBetween,
      space: { main: LengthMetrics.vp(16), cross: LengthMetrics.vp(12) }
    }) {
      Column({ space: 8 }) {
        Text('画质预设')
          .fontSize(16)
          .fontWeight(FontWeight.Medium)
          .fontColor(LauncherColors.text)
        ForEach([this.presetControlEpoch], (_epoch: number) => {
          TabSegmentButtonV2({
            items: this.qualityItems,
            selectedIndex: this.selectedIndex(),
            $selectedIndex: (index: number) => { this.requestPreset(index); },
            itemFontColor: ColorMetrics.resourceColor(LauncherColors.secondaryText),
            itemSelectedFontColor: ColorMetrics.resourceColor(LauncherColors.accent),
            itemSelectedBackgroundColor: ColorMetrics.resourceColor(LauncherColors.surface),
            buttonBackgroundColor: ColorMetrics.resourceColor(LauncherColors.border),
            buttonBorderRadius: LengthMetrics.vp(12),
            itemBorderRadius: LengthMetrics.vp(10),
            itemMinHeight: LengthMetrics.vp(40),
            buttonMinHeight: LengthMetrics.vp(44),
            // Body segments are outside the supported navigation/bar material regions.
            backgroundSystemMaterial: uiMaterial.Material.empty
          })
            .width('100%')
            .enabled(this.settingsEnabled)
        }, (epoch: number) => String(epoch))
      }
      .alignItems(HorizontalAlign.Start)
      .width(200)
      .flexShrink(0)

      Column({ space: 4 }) {
        Row({ space: 12 }) {
          Text('XEngine 超分辨率')
            .fontSize(14)
            .fontColor(LauncherColors.text)
            .layoutWeight(1)
          ForEach([this.superResolutionControlEpoch], (_epoch: number) => {
            Toggle({ type: ToggleType.Switch, isOn: this.superResolution })
              .selectedColor(LauncherColors.accent)
              .systemMaterial(this.controlMaterial)
              .enabled(this.settingsEnabled)
              .accessibilityText('XEngine 超分辨率')
              .onChange((next: boolean) => { this.requestSuperResolution(next); })
          }, (epoch: number) => String(epoch))
        }
        .width('100%')
        .constraintSize({ minHeight: 44 })

        Divider().color(LauncherColors.border)

        Row({ space: 12 }) {
          Text('NNRT/CANN AI 帧插值')
            .fontSize(14)
            .fontColor(LauncherColors.text)
            .layoutWeight(1)
          ForEach([this.frameGenerationControlEpoch], (_epoch: number) => {
            Toggle({ type: ToggleType.Switch, isOn: this.frameGeneration })
              .selectedColor(LauncherColors.accent)
              .systemMaterial(this.controlMaterial)
              .enabled(this.settingsEnabled)
              .accessibilityText('NNRT/CANN AI 帧插值')
              .onChange((next: boolean) => { this.requestFrameGeneration(next); })
          }, (epoch: number) => String(epoch))
        }
        .width('100%')
        .constraintSize({ minHeight: 44 })
      }
      // Bound the percentage-width rows before Flex measures this child's basis.
      .width(288)
      .padding({ left: 14, right: 14, top: 8, bottom: 8 })
      .backgroundColor(LauncherColors.surface)
      .borderRadius(16)
      .flexBasis(288)
      .flexGrow(0)
      .flexShrink(1)
    }
    .width('100%')
  }
}
