/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.protocol.quic.loadbalancer;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.protocol.quic.QuicProxyHandler;

import java.net.InetSocketAddress;

/**
 * L4 QUIC Load Balancer for native QUIC datagram forwarding.
 *
 * <p>This load balancer operates at the UDP datagram layer without QUIC termination.
 * It uses {@link QuicProxyHandler} to forward raw QUIC datagrams between clients and
 * backend servers transparently, preserving end-to-end QUIC encryption.</p>
 *
 * <p>Unlike HTTP/3 load balancing which terminates QUIC and processes HTTP/3 frames,
 * this L4 approach simply routes datagrams based on client address affinity. The proxy
 * does not inspect packet contents, manage QUIC streams, or modify TLS state.</p>
 *
 * <h3>Use Cases</h3>
 * <ul>
 *   <li>Transparent QUIC proxy for any QUIC-based application protocol</li>
 *   <li>QUIC load balancing without TLS termination overhead</li>
 *   <li>Backend QUIC server distribution with session affinity</li>
 * </ul>
 */
public class QuicLoadBalancer extends L4LoadBalancer {

    QuicLoadBalancer(String name, InetSocketAddress bindAddress, L4FrontListener l4FrontListener,
                     ConfigurationContext configurationContext) {
        super(name, bindAddress, l4FrontListener, configurationContext, null);
    }

    @Override
    public String type() {
        return "L4/QUIC";
    }
}
