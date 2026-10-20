import game from 'libharmony_probe.so';
import { KEYBOARD_PARTITIONS, KeyboardKey, KeyboardRow } from './KeyboardLayout';

@Component
export struct GameKeyboard {
  @Prop @Watch('releaseAll') resetEpoch: number = 0;
  @Prop @Watch('onEnabledChanged') inputEnabled: boolean = true;
  onClose: () => void = () => {};
  @State private partition: number = 0;
  @State private holdKeys: boolean = false;
  @State private pressed: string[] = [];
  @State private mouseButtons: number[] = [];
  @Prop viewWidth: number = 720;
  @Prop viewHeight: number = 360;
  private padId: number = -1;
  private padX: number = 0;
  private padY: number = 0;
  private cursorX: number = 400;
  private cursorY: number = 200;

  aboutToDisappear(): void { this.releaseAll(); }

  private onEnabledChanged(): void {
    if (!this.inputEnabled) { this.releaseAll(); }
  }

  private releaseAll(): void {
    this.pressed.forEach((code: string) => { game.sendRawKey(code, false, `keyboard:${code}`); });
    this.mouseButtons.forEach((button: number) => { game.sendMouseButton(button, false, 'keyboard:mouse'); });
    this.pressed = [];
    this.mouseButtons = [];
    this.padId = -1;
    this.holdKeys = false;
  }

  private keyEvent(code: string, event: TouchEvent): void {
    event.stopPropagation();
    if (!this.inputEnabled) { return; }
    if (event.type === TouchType.Down) {
      game.setInputFocus(true);
      const held: boolean = this.pressed.includes(code);
      if (this.holdKeys && held) {
        game.sendRawKey(code, false, `keyboard:${code}`);
        this.pressed = this.pressed.filter((value: string) => value !== code);
      } else if (!held) {
        game.sendRawKey(code, true, `keyboard:${code}`);
        this.pressed = this.pressed.concat([code]);
      }
    } else if (event.type === TouchType.Cancel || (event.type === TouchType.Up && !this.holdKeys)) {
      game.sendRawKey(code, false, `keyboard:${code}`);
      this.pressed = this.pressed.filter((value: string) => value !== code);
    }
  }

  private mouseEvent(button: number, event: TouchEvent): void {
    event.stopPropagation();
    if (!this.inputEnabled) { return; }
    if (event.type === TouchType.Down) {
      game.setInputFocus(true);
      if (this.holdKeys && this.mouseButtons.includes(button)) {
        game.sendMouseButton(button, false, 'keyboard:mouse');
        this.mouseButtons = this.mouseButtons.filter((value: number) => value !== button);
      } else {
        game.sendMouseButton(button, true, 'keyboard:mouse');
        if (!this.mouseButtons.includes(button)) { this.mouseButtons = this.mouseButtons.concat([button]); }
      }
    } else if (event.type === TouchType.Cancel || (event.type === TouchType.Up && !this.holdKeys)) {
      game.sendMouseButton(button, false, 'keyboard:mouse');
      this.mouseButtons = this.mouseButtons.filter((value: number) => value !== button);
    }
  }

  private touchpad(event: TouchEvent): void {
    event.stopPropagation();
    if (!this.inputEnabled) { return; }
    if (event.type === TouchType.Down && this.padId < 0 && event.changedTouches.length > 0) {
      const point: TouchObject = event.changedTouches[0];
      this.padId = point.id;
      this.padX = point.x;
      this.padY = point.y;
      game.setInputFocus(true);
    } else if (event.type === TouchType.Move && this.padId >= 0) {
      const point: TouchObject | undefined = event.touches.find((item: TouchObject) => item.id === this.padId);
      if (point) {
        const dx: number = this.getUIContext().vp2px(point.x - this.padX);
        const dy: number = this.getUIContext().vp2px(point.y - this.padY);
        this.cursorX = Math.max(0, Math.min(this.getUIContext().vp2px(this.viewWidth), this.cursorX + dx));
        this.cursorY = Math.max(0, Math.min(this.getUIContext().vp2px(this.viewHeight), this.cursorY + dy));
        game.sendPointer(this.cursorX, this.cursorY, dx, dy, false);
        this.padX = point.x;
        this.padY = point.y;
      }
    } else if (event.type === TouchType.Cancel ||
      (event.type === TouchType.Up && event.changedTouches.some((item: TouchObject) => item.id === this.padId))) {
      this.padId = -1;
    }
  }

  @Builder
  private keycap(key: KeyboardKey) {
    Text(key.label).width(key.width).height(44).fontSize(12).fontWeight(FontWeight.Medium)
      .fontColor(this.pressed.includes(key.code) ? '#102A22' : '#F3F7F6')
      .backgroundColor(this.pressed.includes(key.code) ? '#B5EAD3' : '#D52B3639')
      .borderRadius(9).border({ width: 1, color: this.pressed.includes(key.code) ? '#C7F8E2' : '#35FFFFFF' })
      .textAlign(TextAlign.Center).accessibilityText(key.code)
      .onTouch((event: TouchEvent) => { this.keyEvent(key.code, event); })
  }

  build() {
    Column({ space: 10 }) {
      Row({ space: 8 }) {
        Column({ space: 2 }) {
          Text('全键盘按键').fontColor('#F5F9F7').fontSize(16).fontWeight(FontWeight.Bold)
          Text(`US 104 · 已按住 ${this.pressed.length} 键`).fontColor('#ABC1B8').fontSize(10)
        }.alignItems(HorizontalAlign.Start)
        Blank()
        Button(this.holdKeys ? '保持：开' : '保持按键').height(44).fontSize(12).focusable(false)
          .backgroundColor(this.holdKeys ? '#B5EAD3' : '#374448').fontColor(this.holdKeys ? '#102A22' : '#F3F7F6')
          .onClick(() => { const next: boolean = !this.holdKeys; this.releaseAll(); this.holdKeys = next; })
        Button('释放全部').height(44).fontSize(12).focusable(false)
          .backgroundColor('#374448').fontColor('#F3F7F6').onClick(() => { this.releaseAll(); })
        Button('×', { type: ButtonType.Circle }).width(44).height(44).fontSize(23).focusable(false)
          .backgroundColor('#374448').fontColor('#F3F7F6').accessibilityText('关闭全键盘')
          .onClick(() => { this.releaseAll(); this.onClose(); })
      }.width('100%')
      Row({ space: 6 }) {
        ForEach(['主键区', '导航区', '数字小键盘', '鼠标区'], (name: string, index: number) => {
          Button(name).height(44).layoutWeight(1).fontSize(12).focusable(false)
            .backgroundColor(this.partition === index ? '#D7ECE3' : '#283539')
            .fontColor(this.partition === index ? '#153C2D' : '#BFD0C9')
            .onClick(() => { if (!this.holdKeys) { this.releaseAll(); } this.padId = -1; this.partition = index; })
        })
      }.width('100%')
      Scroll() {
        if (this.partition < 3) {
          Scroll() {
            Column({ space: 5 }) {
              ForEach(KEYBOARD_PARTITIONS[this.partition].rows, (row: KeyboardRow) => {
                Row({ space: 4 }) {
                  ForEach(row.keys, (key: KeyboardKey) => { this.keycap(key) }, (key: KeyboardKey) => key.code)
                }.width('100%')
              }, (row: KeyboardRow) => row.id)
            }.width(this.partition === 0 ? 804 : '100%').alignItems(HorizontalAlign.Start)
          }.scrollable(ScrollDirection.Horizontal).scrollBar(BarState.Auto).width('100%')
        } else {
          Column({ space: 8 }) {
            Text('滑动移动光标 · 可同时按住修饰键与鼠标键').fontSize(11).fontColor('#ABC1B8')
            Text('触 控 板').width('100%').height(104).textAlign(TextAlign.Center)
              .fontColor('#7D9C90').backgroundColor('#202D30').borderRadius(12)
              .border({ width: 1, color: '#35FFFFFF' })
              .onTouch((event: TouchEvent) => { this.touchpad(event); })
            Row({ space: 6 }) {
              ForEach(['左键', '中键', '右键'], (label: string, button: number) => {
                Text(label).height(44).layoutWeight(1).fontSize(13).fontColor('#F3F7F6').textAlign(TextAlign.Center)
                  .backgroundColor(this.mouseButtons.includes(button) ? '#608B79' : '#374448').borderRadius(9)
                  .onTouch((event: TouchEvent) => { this.mouseEvent(button, event); })
              })
              Button('滚轮 ↑').height(44).fontSize(12).focusable(false).onClick(() => { game.sendWheel(1); })
              Button('滚轮 ↓').height(44).fontSize(12).focusable(false).onClick(() => { game.sendWheel(-1); })
            }.width('100%')
          }.width('100%')
        }
      }.width('100%').layoutWeight(1).scrollBar(BarState.Auto)
    }.padding(14).width('100%').height(Math.min(380, this.viewHeight - 88))
      .backgroundColor('#F2182327').borderRadius(20).border({ width: 1, color: '#35FFFFFF' })
      .shadow({ radius: 20, color: '#66000000', offsetY: 6 })
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
      .onTouch((event: TouchEvent) => { event.stopPropagation(); })
  }
}
