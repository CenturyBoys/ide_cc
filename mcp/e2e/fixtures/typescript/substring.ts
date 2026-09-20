// Bug 2 (relatório rename): "Result" é SUBSTRING de "RefundResult" na MESMA linha (linha 6).
// Resolver "Result" deve atingir a PROPRIEDADE (poucas refs), NÃO o TIPO (muitas).
export class RefundResult {
  code = 0;
}
export class Box extends RefundResult { Result = 1; }
export function useType(): RefundResult {
  return new RefundResult();
}
export function useProp(b: Box): number {
  return b.Result;
}
