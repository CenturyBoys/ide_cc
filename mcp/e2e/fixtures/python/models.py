from dataclasses import dataclass


# ARMADILHA P1 (relatório pachamama): o basedpyright reporta a posição desta classe na linha do
# @dataclass, não do identificador. Sem a correção, find_references(ResponseModel) sem `line` dava 0.
@dataclass(slots=True, frozen=True)
class ResponseModel:
    code: int

    # método DENTRO de classe decorada + o próprio método é decorado (stacked): exercita o scan
    # de identificador pulando decorators, e o find_symbol("ResponseModel/make").
    @classmethod
    def make(cls, code: int) -> "ResponseModel":
        return cls(code=code)


# classe SEM decorator (controle): sempre resolveu, mesmo antes da correção.
class ResponseCode:
    OK = 200
    ERROR = 500
