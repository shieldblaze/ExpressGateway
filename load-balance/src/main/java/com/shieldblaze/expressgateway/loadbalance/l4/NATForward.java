package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.backend.Backend;

import java.util.List;

/**
 * Select a single {@link Backend}. Used for NAT-Forwarding purpose.
 */
public final class NATForward extends L4Balance {

    public NATForward() {
        super(new NOOPSessionPersistence());
    }

    /**
     * @param backends {@link List} of {@link Backend}
     * @see #setBackends(List)
     */
    public NATForward(List<Backend> backends) {
        super(new NOOPSessionPersistence());
        setBackends(backends);
    }

    /**
     * @param backends {@link List} of {@link Backend}
     * @throws IllegalArgumentException If {@link List} of {@link Backend} is more than 1
     * @see L4Balance#setBackends(List)
     */
    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        if (backends.size() > 1) {
            throw new IllegalArgumentException("Backends Cannot Be More Than 1 (one).");
        }
    }

    @Override
    public L4Response getResponse(L4Request l4Request) {
        return new L4Response(backends.get(0));
    }
}
