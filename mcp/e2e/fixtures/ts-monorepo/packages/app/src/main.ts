// package app — importa Thing de OUTRO package pelo alias @core (cross-package).
import { Thing } from "@core/index";

export function run(): string {
  const t = new Thing();
  return t.greet();
}
