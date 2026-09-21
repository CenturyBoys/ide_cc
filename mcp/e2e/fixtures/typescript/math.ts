// Função top-level para exercitar move_symbol (cria arquivo novo em TS/vtsls) e extract_function.
export function addNumbers(a: number, b: number): number {
  const sum = a + b;
  return sum;
}

export function useMath(): number {
  // dois call-sites do MESMO chamador (P14: call_site_count deve ser 2, não 1)
  return addNumbers(2, 3) + addNumbers(4, 5);
}
