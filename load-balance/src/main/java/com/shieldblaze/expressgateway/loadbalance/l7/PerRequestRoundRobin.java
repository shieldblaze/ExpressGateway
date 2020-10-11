package com.shieldblaze.expressgateway.loadbalance.l7;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.SessionPersistence;
import io.netty.handler.codec.http.HttpRequest;

import java.util.List;

public final class PerRequestRoundRobin extends L7Balance {

    private final boolean enableHTTP2;
    private RoundRobinImpl<Backend> backendsRoundRobin;

    public PerRequestRoundRobin() {
        super(new NOOPSessionPersistence());
        enableHTTP2 = true;
    }

    public PerRequestRoundRobin(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends, true);
    }

    public PerRequestRoundRobin(SessionPersistence sessionPersistence, List<Backend> backends, boolean enableHTTP2) {
        super(sessionPersistence);
        setBackends(backends);
        this.enableHTTP2 = enableHTTP2;
    }

    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        backendsRoundRobin = new RoundRobinImpl<>(this.backends);
    }

    @Override
    public Backend getBackend(HttpRequest httpRequest) {
        Backend backend = sessionPersistence.getBackend(httpRequest);
        if (backend != null) {
            return backend;
        }

        backend = backendsRoundRobin.iterator().next();
        sessionPersistence.addRoute(httpRequest, backend);
        return backend;
    }
}
