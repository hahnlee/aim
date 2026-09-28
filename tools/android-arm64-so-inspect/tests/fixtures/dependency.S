.text
.p2align 2
.global external_api
.type external_api, %function
external_api:
    add w0, w0, #1
    ret
.size external_api, .-external_api

