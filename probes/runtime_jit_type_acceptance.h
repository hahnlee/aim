#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitTypes(JNIEnv* env,art::Thread* self,art::jit::Jit* jit,
                          art::Handle<art::mirror::Class> owner,jclass java_owner) {
  jclass string_class=env->FindClass("java/lang/String");
  jmethodID base_id=env->GetStaticMethodID(java_owner,"jitVirtualBaseClass","()Ljava/lang/Class;");
  jmethodID child_id=env->GetStaticMethodID(java_owner,"jitVirtualChildClass","()Ljava/lang/Class;");
  if(!base_id || !child_id || env->ExceptionCheck()) return false;
  jclass base_class=static_cast<jclass>(env->CallStaticObjectMethod(java_owner,base_id));
  if(!base_class || env->ExceptionCheck()) return false;
  jclass child_class=static_cast<jclass>(env->CallStaticObjectMethod(java_owner,child_id));
  if(!child_class || env->ExceptionCheck()) return false;
  jclass number_class=env->FindClass("java/lang/Number");
  jclass integer_class=env->FindClass("java/lang/Integer");
  jclass sequence_class=env->FindClass("java/lang/CharSequence");
  jclass serializable_class=env->FindClass("java/io/Serializable");
  if(!sequence_class || !serializable_class || env->ExceptionCheck()) return false;
  if(!base_class || !child_class || !number_class || !integer_class || env->ExceptionCheck()) return false;
  jclass cast_error=env->FindClass("java/lang/ClassCastException");
  jobject text=env->NewStringUTF("JIT type boundary");
  jobject object=env->AllocObject(java_owner);
  jobject array=env->NewIntArray(3);
  jclass object_class=env->FindClass("java/lang/Object");
  jclass objects_class=env->FindClass("[Ljava/lang/Object;");
  jclass strings_class=env->FindClass("[Ljava/lang/String;");
  jclass ints_class=env->FindClass("[I");
  jclass sequences_class=env->FindClass("[Ljava/lang/CharSequence;");
  if(!object_class || !objects_class || !strings_class || !ints_class || !sequences_class || env->ExceptionCheck()) return false;
  jobject strings=env->NewObjectArray(2,string_class,nullptr), objects=env->NewObjectArray(2,object_class,nullptr);
  jobject integers=env->NewObjectArray(2,integer_class,nullptr), nested=env->NewObjectArray(2,ints_class,nullptr);
  jobject longs=env->NewLongArray(2);
  if(!strings || !objects || !integers || !nested || !longs || env->ExceptionCheck()) return false;
  jobject base=env->AllocObject(base_class), child=env->AllocObject(child_class), integer=env->AllocObject(integer_class);
  if(!base || !child || !integer || env->ExceptionCheck()) return false;
  if(!string_class || !cast_error || !text || !object || !array || env->ExceptionCheck()) return false;
  const char* names[]={"jitIsString","jitCastString","jitIsBase","jitCastBase","jitIsNumber","jitCastNumber",
    "jitIsSequence","jitCastSequence","jitIsSerializable","jitCastSerializable",
    "jitIsObjects","jitCastObjects","jitIsStrings","jitCastStrings","jitIsInts","jitCastInts","jitIsSequences","jitCastSequences"};
  jclass targets[]={string_class,base_class,number_class,sequence_class,serializable_class,
    objects_class,strings_class,ints_class,sequences_class};
  const char* sigs[]={"(Ljava/lang/Object;)Z","(Ljava/lang/Object;)Ljava/lang/Object;"};
  unsigned cases=0;
  for(unsigned kind=0;kind<18;++kind) {
    auto* method=owner->FindClassMethod(names[kind],sigs[kind%2],art::kRuntimePointerSize);
    jmethodID id=env->GetStaticMethodID(java_owner,names[kind],sigs[kind%2]);
    if(!method || !id || env->ExceptionCheck() ||
       jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    for(unsigned phase=0;phase<2;++phase) {
      if(phase==1 && (!jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) ||
         !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) return false;
      for(jobject value:{jobject(nullptr),text,object,array,base,child,integer,strings,objects,integers,nested,longs}) {
        bool matches=value && env->IsInstanceOf(value,targets[kind/2]);
        if(kind%2==0) {
          bool actual=env->CallStaticBooleanMethod(java_owner,id,value);
          if(env->ExceptionCheck() || actual!=matches) return false;
        } else {
          jobject actual=env->CallStaticObjectMethod(java_owner,id,value);
          bool should_throw=value && !matches;
          if(bool(env->ExceptionCheck())!=should_throw) return false;
          if(should_throw) {
            jthrowable error=env->ExceptionOccurred(); env->ExceptionClear();
            bool correct=env->IsInstanceOf(error,cast_error); env->DeleteLocalRef(error);
            if(!correct) return false;
          } else if(!env->IsSameObject(actual,value)) return false;
          if(actual) env->DeleteLocalRef(actual);
        }
        ++cases;
      }
    }
  }
  env->DeleteLocalRef(text); env->DeleteLocalRef(object); env->DeleteLocalRef(array);
  env->DeleteLocalRef(string_class); env->DeleteLocalRef(cast_error);
  env->DeleteLocalRef(sequence_class); env->DeleteLocalRef(serializable_class);
  for(jobject ref:{jobject(object_class),jobject(objects_class),jobject(strings_class),jobject(ints_class),jobject(sequences_class),
      strings,objects,integers,nested,longs}) env->DeleteLocalRef(ref);
  for(jobject ref:{jobject(base_class),jobject(child_class),jobject(number_class),jobject(integer_class),base,child,integer}) env->DeleteLocalRef(ref);
  std::cerr<<"ART JIT types: class/interface/array covariance, instanceof/cast and exceptions PASS cases="<<cases<<"\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
