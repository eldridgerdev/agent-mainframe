import { expect } from "vitest";

export type Rgb = [number, number, number];

export function blocks(css: string): { light: string; dark: string } {
  const dark = css.indexOf("@media (prefers-color-scheme: dark)");
  expect(dark).toBeGreaterThan(0);
  return { light: css.slice(0, dark), dark: css.slice(dark) };
}

export function token(block: string, name: string): string {
  const match = block.match(new RegExp(`--${name}:\\s*([^;]+);`));
  expect(match, `--${name}`).toBeTruthy();
  return match![1].trim();
}

export function parse(color: string): { rgb: Rgb; alpha: number } {
  const hex = color.match(/^#([0-9a-f]{6})$/i);
  if (hex) return { rgb: [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16)) as Rgb, alpha: 1 };
  const rgba = color.match(/^rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\s*\)$/);
  expect(rgba, color).toBeTruthy();
  return { rgb: [Number(rgba![1]), Number(rgba![2]), Number(rgba![3])], alpha: Number(rgba![4]) };
}

export function over(color: string, base: Rgb): Rgb {
  const { rgb, alpha } = parse(color);
  return rgb.map((channel, i) => channel * alpha + base[i] * (1 - alpha)) as Rgb;
}

export function luminance(rgb: Rgb): number {
  const [r, g, b] = rgb.map((value) => {
    const channel = value / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

