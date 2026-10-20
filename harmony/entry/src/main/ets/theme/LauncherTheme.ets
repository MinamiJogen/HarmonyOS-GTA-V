import { CustomTheme } from '@kit.ArkUI';

export class LauncherColors {
  static readonly background: string = '#F4F7F5';
  static readonly card: string = '#E4F1E9';
  static readonly surface: string = '#F5FAF7';
  static readonly accent: string = '#23815F';
  static readonly text: string = '#183F34';
  static readonly secondaryText: string = '#59726C';
  static readonly border: string = '#CDDFD4';
  static readonly secondaryButton: string = '#C7E1D0';
  static readonly secondaryButtonBorder: string = '#9FBFAC';
}

// Use ArkUI theme tokens so native buttons, toggles and dialogs share the green accent.
export const launcherTheme: CustomTheme = {
  colors: {
    brand: LauncherColors.accent,
    primary: LauncherColors.accent,
    onPrimary: '#FFFFFF',
    container: LauncherColors.card,
    fontPrimary: LauncherColors.text,
    fontSecondary: LauncherColors.secondaryText,
    fontEmphasize: LauncherColors.accent,
    fontOnPrimary: '#FFFFFF',
    iconPrimary: LauncherColors.text,
    iconEmphasize: LauncherColors.accent,
    backgroundPrimary: LauncherColors.background,
    backgroundSecondary: LauncherColors.surface,
    compBackgroundPrimary: LauncherColors.card,
    compBackgroundSecondary: LauncherColors.secondaryButton,
    compBackgroundEmphasize: LauncherColors.accent,
    compEmphasizeSecondary: LauncherColors.secondaryButton,
    compDivider: LauncherColors.border,
    interactiveActive: LauncherColors.accent,
    interactiveSelect: '#D8EADF'
  }
};
