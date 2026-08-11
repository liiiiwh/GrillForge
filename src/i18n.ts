export type Locale = "zh-CN" | "en-US";

export const DEFAULT_LOCALE: Locale = "zh-CN";
export const SUPPORTED_LOCALES: readonly Locale[] = ["zh-CN", "en-US"];

const messages = {
  "zh-CN": {
    overview: "控制中心",
    clients: "客户端",
    models: "模型",
    providers: "供应商",
    routes: "配置关系",
    native: "跟随原生 / 未指定",
    sonnetSlot: "Sonnet 槽位",
    opusSlot: "Opus 槽位",
    fableSlot: "Fable 槽位",
    haikuSlot: "Haiku 槽位",
  },
  "en-US": {
    overview: "Control center",
    clients: "Clients",
    models: "Models",
    providers: "Providers",
    routes: "Configuration map",
    native: "Native / Unspecified",
    sonnetSlot: "Sonnet slot",
    opusSlot: "Opus slot",
    fableSlot: "Fable slot",
    haikuSlot: "Haiku slot",
  },
} as const;

export type MessageKey = keyof (typeof messages)["zh-CN"];

export function createTranslator(locale: Locale) {
  return (key: MessageKey) => messages[locale][key];
}
