package com.shieldblaze.expressgateway.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoBackendAvailableException;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;

/**
 * Select a single {@link Node}. Used for NAT-Forwarding purpose.
 */
public final class NATForward extends L4Balance {

    private L4Response l4Response;

    public NATForward() {
        super(new NOOPSessionPersistence());
    }

    public NATForward(Cluster cluster) {
        super(new NOOPSessionPersistence());
        cluster(cluster);
    }

    @Override
    public void cluster(Cluster cluster) {
        super.cluster(cluster);
        if (cluster.size() > 1) {
            throw new IllegalArgumentException("Cluster size cannot be more than 1 (one).");
        }
        this.l4Response = new L4Response(cluster.get(0));
    }

    @Override
    public L4Response response(L4Request l4Request) throws LoadBalanceException {
         if (l4Response.backend().state() != State.ONLINE) {
             throw new NoBackendAvailableException("No Backend available for Cluster: " + cluster);
         }
         return l4Response;
    }
}
