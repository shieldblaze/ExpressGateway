package com.shieldblaze.expressgateway.loadbalance.l7;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.SessionPersistence;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.cookie.Cookie;
import io.netty.handler.codec.http.cookie.DefaultCookie;

public final class StickySession extends L7Balance {

    /**
     * Create {@link L7Balance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public StickySession(SessionPersistence sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public Backend getBackend(HttpRequest httpRequest) {
        Cookie cookie = new DefaultCookie("X-Route-ID", "1234");
        return null;
    }
}
