from models import ResponseCode, ResponseModel


def build_ok() -> ResponseModel:
    return ResponseModel.make(ResponseCode.OK)


def build_error() -> ResponseModel:
    return ResponseModel(code=ResponseCode.ERROR)


def describe(r: ResponseModel) -> int:
    return r.code
