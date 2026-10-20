import { common } from '@kit.AbilityKit';
import frameProbe from 'libgraphics_frame_surface.so';

@Entry
@Component
struct GraphicsFrameProbe {
  @StorageProp('fgSurfaceEnabled') private fgEnabled: boolean = true;
  @StorageProp('fgSurfaceSeconds') private seconds: number = 20;
  @StorageProp('fgSurfaceUiPrediction') private uiPrediction: boolean = false;
  @StorageProp('fgSurfaceGeometry') private geometry: boolean = false;
  @StorageProp('fgSurfaceSwapchainSampled') private swapchainSampled: boolean = false;
  @StorageProp('fgSurfaceFinalDepthState') private finalDepthState: boolean = false;
  @StorageProp('fgSurfaceSourceFps') private sourceFps: number = 15;
  @StorageProp('fgSurfaceTargetFps') private targetFps: number = 30;
  @StorageProp('fgSurfaceFusionAwareMetadata') private fusionAwareMetadata: string = 'unavailable';
  @StorageProp('fgSurfaceInspectOnly') private inspectOnly: boolean = false;
  @StorageProp('fgSurfaceRifeStream') private rifeStream: boolean = false;
  @State private details: string = 'Preparing independent Vulkan surface';

  aboutToAppear(): void {
    const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
    if (this.inspectOnly) {
      this.details = frameProbe.inspectPublicInterfaces(context.filesDir);
      return;
    }
    const configured: boolean = frameProbe.configure(context.filesDir, this.fgEnabled, this.seconds,
      this.uiPrediction, this.geometry, this.sourceFps, this.targetFps, this.fusionAwareMetadata,
      this.swapchainSampled, this.finalDepthState, this.rifeStream);
    this.details = configured ? (this.rifeStream ?
      `Live synthetic RIFE 320×192 · source 15 Hz · requested output 30 Hz · 200ms buffer · ${this.seconds}s` :
      `Synthetic 1202×800 · source ${this.sourceFps} Hz · requested output ${this.targetFps} Hz · AI ${this.fgEnabled ? 'ON' : 'OFF'} · FusionAware ${this.fusionAwareMetadata} · ${this.geometry ? 'real 3D+depth' : 'clear bars'} · UI prediction ${this.uiPrediction ? 'ON' : 'OFF'} · sampled swapchain requested ${this.swapchainSampled ? 'ON' : 'OFF'} · disabled final depth state requested ${this.finalDepthState ? 'ON' : 'OFF'} · ${this.seconds}s`) : frameProbe.status();
  }
  aboutToDisappear(): void { frameProbe.stop(); }

  build() {
    Stack({ alignContent: Alignment.TopStart }) {
      if (!this.inspectOnly) {
        XComponent({ id: 'graphics-frame-surface-probe', type: XComponentType.SURFACE,
          libraryname: 'graphics_frame_surface' }).width('100%').height('100%')
      }
      Column({ space: 6 }) {
        Text(this.details).fontSize(14).fontColor(Color.White).maxLines(7)
        Row({ space: 8 }) {
          Button('Read status').onClick(() => { this.details = frameProbe.status(); })
          Button('Stop').onClick(() => { frameProbe.stop(); this.details = frameProbe.status(); })
        }
        Text('API/source counts do not verify generated or displayed frame rate.')
          .fontSize(12).fontColor(Color.White)
        if (!this.rifeStream) {
          Text('SetTargetFps is documented to take effect after first game release. Sideloaded output must be measured.')
            .fontSize(12).fontColor(Color.White)
        }
      }.padding(10).backgroundColor('#99000000').width('70%')
    }.width('100%').height('100%').backgroundColor(Color.Black)
  }
}
