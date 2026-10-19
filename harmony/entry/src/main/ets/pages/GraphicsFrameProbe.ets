import { common } from '@kit.AbilityKit';
import frameProbe from 'libgraphics_frame_surface.so';

@Entry
@Component
struct GraphicsFrameProbe {
  @StorageProp('fgSurfaceEnabled') private fgEnabled: boolean = true;
  @StorageProp('fgSurfaceSeconds') private seconds: number = 20;
  @StorageProp('fgSurfaceUiPrediction') private uiPrediction: boolean = false;
  @StorageProp('fgSurfaceGeometry') private geometry: boolean = false;
  @State private details: string = 'Preparing independent Vulkan surface';

  aboutToAppear(): void {
    const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
    const configured: boolean = frameProbe.configure(context.filesDir, this.fgEnabled, this.seconds, this.uiPrediction, this.geometry);
    this.details = configured ? `Synthetic 1202×800 · source 30 Hz · AI ${this.fgEnabled ? 'ON' : 'OFF'} · ${this.geometry ? 'real 3D+depth' : 'clear bars'} · UI prediction ${this.uiPrediction ? 'ON' : 'OFF'} · ${this.seconds}s` : frameProbe.status();
  }
  aboutToDisappear(): void { frameProbe.stop(); }

  build() {
    Stack({ alignContent: Alignment.TopStart }) {
      XComponent({ id: 'graphics-frame-surface-probe', type: XComponentType.SURFACE,
        libraryname: 'graphics_frame_surface' }).width('100%').height('100%')
      Column({ space: 6 }) {
        Text(this.details).fontSize(14).fontColor(Color.White).maxLines(7)
        Row({ space: 8 }) {
          Button('Read status').onClick(() => { this.details = frameProbe.status(); })
          Button('Stop').onClick(() => { frameProbe.stop(); this.details = frameProbe.status(); })
        }
        Text('API/source counts do not verify generated or displayed frame rate.')
          .fontSize(12).fontColor(Color.White)
      }.padding(10).backgroundColor('#99000000').width('70%')
    }.width('100%').height('100%').backgroundColor(Color.Black)
  }
}
