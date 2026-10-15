package android.content.pm.parsing.result;public interface ParseResult<T>{boolean isError();String getErrorMessage();Exception getException();T getResult();}
