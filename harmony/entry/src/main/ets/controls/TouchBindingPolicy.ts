import { TouchAction } from './TouchLayout';
import { rawTouchBinding } from './RawTouchBindings';

/** Literal input availability is independent of whether the resulting game action was verified. */
export function hasLiteralTouchInput(action: TouchAction): boolean {
  return action.rawKey.length > 0 || rawTouchBinding(action.control) !== undefined;
}
export function isDirectTouchAction(action: TouchAction): boolean {
  return hasLiteralTouchInput(action) || action.uiAction === 'manual_mode_picker' || action.gesture === 'open_keyboard';
}
