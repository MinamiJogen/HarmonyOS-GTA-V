import { RawTouchBinding, RawTouchAxis, rawTouchAxis, rawTouchBinding } from './RawTouchBindings';

export interface DirectionButton {
  id: string;
  direction: string;
  label: string;
  control: string;
  binding: string;
  mapping?: RawTouchBinding;
  mouseAxis?: string;
  mouseSign?: number;
}

/** Discrete holds use exactly the PC mappings previously selected by each axis. */
export function directionButtons(axisX: string, axisY: string, directions: string[]): DirectionButton[] {
  const result: DirectionButton[] = [];
  const positions: string[] = ['left', 'right', 'up', 'down'];
  const arrows: string[] = ['←', '→', '↑', '↓'];
  if (directions.length === 4) {
    for (let i = 0; i < 4; i++) {
      const mapping: RawTouchBinding | undefined = rawTouchBinding(directions[i]);
      result.push({ id: positions[i], direction: positions[i], label: mapping ? mapping.label : arrows[i],
        control: directions[i], binding: `axis:${directions[i]}`, mapping: mapping });
    }
    return result;
  }
  const axes: string[] = [axisX, axisY];
  for (let i = 0; i < axes.length; i++) {
    if (axes[i].length === 0) { continue; }
    const axis: RawTouchAxis | undefined = rawTouchAxis(axes[i]);
    // These two engine controls explicitly use the existing relative mouse
    // axes (Cfx control reference66/67). Other unknown axes remain unavailable.
    const mouseAxis: string | undefined = axes[i] === 'INPUT_VEH_GUN_LR' ? 'x' :
      (axes[i] === 'INPUT_VEH_GUN_UD' ? 'y' : undefined);
    if (!axis && !mouseAxis) { continue; }
    for (let sign = 0; sign < 2; sign++) {
      const n: number = i * 2 + sign;
      const control: string = axis ? (sign === 0 ? axis.negative : axis.positive) : axes[i];
      const mapping: RawTouchBinding | undefined = rawTouchBinding(control);
      result.push({ id: positions[n], direction: positions[n], label: mapping ? mapping.label : arrows[n],
        control: control, binding: `axis:${axes[i]}:${sign === 0 ? 'negative' : 'positive'}`, mapping: mapping,
        mouseAxis: mouseAxis, mouseSign: mouseAxis ? (sign === 0 ? -1 : 1) : undefined });
    }
  }
  return result;
}
