package com.shieldblaze.expressgateway.loadbalance.l7;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.SessionPersistence;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.util.internal.ObjectUtil;

import java.util.ArrayList;
import java.util.List;

public abstract class L7Balance {
    protected SessionPersistence sessionPersistence;
    protected List<Backend> backends;

    /**
     * Create {@link L7Balance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public L7Balance(SessionPersistence sessionPersistence) {
        this.sessionPersistence = ObjectUtil.checkNotNull(sessionPersistence, "Session Persistence");
    }

    /**
     * Set Backends
     *
     * @param backends {@link List} of {@link Backend}
     * @throws IllegalArgumentException If {@link List} of {@link Backend} is Empty.
     * @throws NullPointerException     If {@link List} of {@link Backend} is {@code null}.
     */
    public void setBackends(List<Backend> backends) {
        ObjectUtil.checkNotNull(backends, "Backend List");
        if (backends.size() == 0) {
            throw new IllegalArgumentException("Backends List Cannot Be Empty");
        }
        this.backends = new ArrayList<>(backends);

    }

    public abstract Backend getBackend(HttpRequest httpRequest);
}
