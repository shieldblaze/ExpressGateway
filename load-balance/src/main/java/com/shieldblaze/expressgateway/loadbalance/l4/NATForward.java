package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.loadbalance.exceptions.NoBackendAvailableException;

/**
 * Select a single {@link Backend}. Used for NAT-Forwarding purpose.
 */
public final class NATForward extends L4Balance {

    private L4Response l4Response;

    public NATForward() {
        super(new NOOPSessionPersistence());
    }

    public NATForward(Cluster cluster) {
        super(new NOOPSessionPersistence());
        setCluster(cluster);
    }

    @Override
    public void setCluster(Cluster cluster) {
        super.setCluster(cluster);
        if (cluster.size() > 1) {
            throw new IllegalArgumentException("Cluster size cannot be more than 1 (one).");
        }
        this.l4Response = new L4Response(cluster.get(0));
    }

    @Override
    public L4Response getResponse(L4Request l4Request) throws LoadBalanceException {
         if (l4Response.getBackend().getState() != State.ONLINE) {
             throw new NoBackendAvailableException("No Backend available for Cluster: " + cluster);
         }
         return l4Response;
    }
}
