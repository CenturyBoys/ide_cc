from dataclasses import dataclass


@dataclass(slots=True, frozen=True)
class ResponseModel:
    code: int

    @classmethod
    def make(cls, code: int) -> "ResponseModel":
        return cls(code=code)

    # renomear make -> build COLIDE (irmão já existe). Sob typeCheckingMode=basic, a detecção de
    # colisão de escopo (independente de diagnósticos) tem que barrar do mesmo jeito que sob "off".
    @classmethod
    def build(cls, code: int) -> "ResponseModel":
        return cls(code=code)


def use() -> ResponseModel:
    return ResponseModel.make(1)
