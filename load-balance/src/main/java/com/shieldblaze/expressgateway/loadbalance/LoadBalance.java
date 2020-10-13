package com.shieldblaze.expressgateway.loadbalance;

import com.shieldblaze.expressgateway.backend.Backend;

import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/**
 * Base Implementation for Load Balance
 * @param <R> Session Persistence Get Type
 * @param <K> Session Persistence Key Type
 * @param <V> Session Persistence Value Type
 */
public abstract class LoadBalance<R, K, V> {
    protected SessionPersistence<R, K, V> sessionPersistence;
    protected List<Backend> backends;

    /**
     * Create {@link LoadBalance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public LoadBalance(SessionPersistence<R, K, V> sessionPersistence) {
        this.sessionPersistence = Objects.requireNonNull(sessionPersistence, "sessionPersistence");
    }

    /**
     * Set Backends
     *
     * @param backends {@link List} of {@link Backend}
     * @throws IllegalArgumentException If {@link List} of {@link Backend} is Empty.
     * @throws NullPointerException     If {@link List} of {@link Backend} is {@code null}.
     */
    public void setBackends(List<Backend> backends) {
        Objects.requireNonNull(backends, "backends");
        if (backends.size() == 0) {
            throw new IllegalArgumentException("Backends List Cannot Be Empty");
        }
        this.backends = new ArrayList<>(backends);
    }

    public abstract Response getResponse(Request request);
}
