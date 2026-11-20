// SPDX-License-Identifier: GPL-3.0-or-later
import enUS from "./locales/en-US";
import esES from "./locales/es-ES";

const locales: Record<string, typeof enUS> = {
  "en-US": enUS,
  "es-ES": esES,
};

export function i18n(locale: string) {
  return locales[locale] ?? locales[locale.split("-")[0] === "es" ? "es-ES" : "en-US"] ?? enUS;
}
