import { currentLocale, type Locale } from './index'

function active(locale?: Locale): Locale {
  return locale ?? currentLocale()
}

export function formatNumber(
  value: number,
  options?: Intl.NumberFormatOptions,
  locale?: Locale,
): string {
  return new Intl.NumberFormat(active(locale), options).format(value)
}

export function formatPercent(value: number, locale?: Locale): string {
  return formatNumber(value, { style: 'percent', maximumFractionDigits: 0 }, locale)
}

export function formatDateTime(
  value: Date | number | string,
  locale?: Locale,
): string {
  const date = value instanceof Date ? value : new Date(value)
  return new Intl.DateTimeFormat(active(locale), {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(date)
}

export function formatDuration(milliseconds: number, locale?: Locale): string {
  const seconds = Math.max(0, milliseconds) / 1000
  if (seconds < 60) {
    return formatNumber(seconds, {
      style: 'unit',
      unit: 'second',
      unitDisplay: 'short',
      maximumFractionDigits: seconds < 10 ? 1 : 0,
    }, locale)
  }
  return formatNumber(seconds / 60, {
    style: 'unit',
    unit: 'minute',
    unitDisplay: 'short',
    maximumFractionDigits: 1,
  }, locale)
}

export function formatBytes(bytes: number, locale?: Locale): string {
  const safe = Math.max(0, bytes)
  const units = ['B', 'KB', 'MB', 'GB', 'TB'] as const
  const index = safe === 0 ? 0 : Math.min(units.length - 1, Math.floor(Math.log(safe) / Math.log(1024)))
  const value = safe / 1024 ** index
  return `${formatNumber(value, { maximumFractionDigits: value < 10 && index > 0 ? 1 : 0 }, locale)} ${units[index]}`
}
