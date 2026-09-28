.text
.p2align 2
.global exported_add
.type exported_add, %function
exported_add:
    add w0, w0, w1
    ret
.size exported_add, .-exported_add

.global calls_external_api
.type calls_external_api, %function
calls_external_api:
    bl external_api
    ret
.size calls_external_api, .-calls_external_api

.global smoke_constructor
.type smoke_constructor, %function
smoke_constructor:
    ret
.size smoke_constructor, .-smoke_constructor

.section .init_array,"aw",%init_array
.p2align 3
.xword smoke_constructor

.section .tdata,"awT",%progbits
.p2align 2
.global tls_counter
.type tls_counter, %tls_object
tls_counter:
    .word 7
.size tls_counter, .-tls_counter

.data
.p2align 3
.global exported_data
.type exported_data, %object
exported_data:
    .xword 0x1122334455667788
.size exported_data, .-exported_data
