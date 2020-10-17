package com.shieldblaze.expressgateway.core.loadbalancer.l7.http;

import com.shieldblaze.expressgateway.backend.Cluster;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.server.http.HTTPFrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;

import java.net.InetSocketAddress;

/**
 * Default implementation of {@link HTTPLoadBalancer}
 */
final class DefaultHTTPLoadBalancer extends HTTPLoadBalancer {

    DefaultHTTPLoadBalancer(InetSocketAddress bindAddress, HTTPBalance HTTPBalance, HTTPFrontListener httpFrontListener, Cluster cluster,
                            CommonConfiguration commonConfiguration, HTTPConfiguration httpConfiguration) {
        super(bindAddress, HTTPBalance, httpFrontListener, cluster, commonConfiguration, httpConfiguration);
    }
}
