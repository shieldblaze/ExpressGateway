package com.shieldblaze.expressgateway.core.loadbalancer.l7.http;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.connection.ConnectionManager;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;

import java.net.InetSocketAddress;

/**
 * Default implementation of {@link HTTPLoadBalancer}
 */
final class DefaultHTTPLoadBalancer extends HTTPLoadBalancer {

    /**
     * @param bindAddress         {@link InetSocketAddress} on which {@link L7FrontListener} will bind and listen.
     * @param HTTPBalance         {@link HTTPBalance} for Load Balance
     * @param l7FrontListener     {@link L7FrontListener} for listening and handling traffic
     * @param clusterPool         {@link Cluster} to be Load Balanced
     * @param commonConfiguration {@link CommonConfiguration} to be applied
     * @param connectionManager   {@link ConnectionManager} to use
     * @param httpConfiguration   {@link HTTPConfiguration} to be applied
     * @throws NullPointerException If any parameter is {@code null}
     */
    DefaultHTTPLoadBalancer(InetSocketAddress bindAddress, HTTPBalance HTTPBalance, L7FrontListener l7FrontListener, Cluster cluster,
                            CommonConfiguration commonConfiguration, ConnectionManager connectionManager, HTTPConfiguration httpConfiguration) {
        super(bindAddress, HTTPBalance, l7FrontListener, cluster, commonConfiguration, connectionManager, httpConfiguration);
    }
}
