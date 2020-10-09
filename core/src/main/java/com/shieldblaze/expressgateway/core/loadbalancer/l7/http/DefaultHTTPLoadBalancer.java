package com.shieldblaze.expressgateway.core.loadbalancer.l7.http;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.server.http.HTTPFrontListener;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;

import java.net.InetSocketAddress;

/**
 * Default implementation of {@link HTTPLoadBalancer}
 */
final class DefaultHTTPLoadBalancer extends HTTPLoadBalancer {

    DefaultHTTPLoadBalancer(InetSocketAddress bindAddress, L7Balance l7Balance, HTTPFrontListener httpFrontListener, Cluster cluster,
                            CommonConfiguration commonConfiguration, HTTPConfiguration httpConfiguration) {
        super(bindAddress, l7Balance, httpFrontListener, cluster, commonConfiguration, httpConfiguration);
    }
}
