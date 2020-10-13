package com.shieldblaze.expressgateway.core.loadbalancer.l7.http;

import com.shieldblaze.expressgateway.backend.Cluster;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.server.http.HTTPFrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPL7Balance;

import java.net.InetSocketAddress;

/**
 * Default implementation of {@link HTTPLoadBalancer}
 */
final class DefaultHTTPLoadBalancer extends HTTPLoadBalancer {

    DefaultHTTPLoadBalancer(InetSocketAddress bindAddress, HTTPL7Balance HTTPL7Balance, HTTPFrontListener httpFrontListener, Cluster cluster,
                            CommonConfiguration commonConfiguration, HTTPConfiguration httpConfiguration) {
        super(bindAddress, HTTPL7Balance, httpFrontListener, cluster, commonConfiguration, httpConfiguration);
    }
}
