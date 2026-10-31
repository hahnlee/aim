package android.accounts;
public interface AccountManagerFuture<T> { T getResult(long timeout, java.util.concurrent.TimeUnit unit) throws Exception; }
