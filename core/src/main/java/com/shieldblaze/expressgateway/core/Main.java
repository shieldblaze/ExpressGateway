/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.CommonConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.core.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.core.configuration.tls.MutualTLS;
import com.shieldblaze.expressgateway.core.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSServerMapping;
import com.shieldblaze.expressgateway.core.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.core.server.http.HTTPListener;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l7.RoundRobin;
import io.netty.handler.codec.http2.Http2CodecUtil;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.handler.ssl.util.SelfSignedCertificate;

import javax.net.ssl.SSLException;
import java.net.InetSocketAddress;
import java.security.cert.CertificateException;
import java.util.Collections;

public final class Main {

    static {
        System.setProperty("log4j.configurationFile", "log4j2.xml");
//        System.load("/home/aayush/Documents/ExpressGateway/bin/libnetty_tcnative.so");
    }

    public static void main(String[] args) throws CertificateException, SSLException {
        TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withTCPFastOpenMaximumPendingRequests(2147483647)
                .withBackendConnectTimeout(10000 * 5)
                .withBackendSocketTimeout(10000 * 5)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{100})
                .withSocketReceiveBufferSize(2147483647)
                .withSocketSendBufferSize(2147483647)
                .withTCPConnectionBacklog(2147483647)
                .withDataBacklog(2147483647)
                .withConnectionIdleTimeout(1800000)
                .build();

        EventLoopConfiguration eventLoopConfiguration = EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(2)
                .withChildWorkers(4)
                .build();

        CommonConfiguration configuration = CommonConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration.DEFAULT)
                .build();

        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(
                Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key(),false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);

        TLSConfiguration tlsConfiguration = TLSConfigurationBuilder.forServer()
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_128_GCM_SHA256))
                .withUseALPN(true)
                .withTLSServerMapping(tlsServerMapping)
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .build();

        TLSConfiguration forClient = TLSConfigurationBuilder.forClient()
                .withProtocols(Collections.singletonList(Protocol.TLS_1_2))
                .withCiphers(Collections.singletonList(Cipher.TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256))
                .withUseALPN(true)
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withTrustManager(InsecureTrustManagerFactory.INSTANCE.getTrustManagers()[0])
                .build();

        Cluster cluster = new Cluster();
        cluster.setClusterName("MyCluster");
        cluster.addBackend(new Backend("www.google.com", new InetSocketAddress("www.google.com", 443)));
//        cluster.addBackend(new Backend("one.one.one.one", new InetSocketAddress("one.one.one.one", 443)));

/*        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withConfiguration(configuration)
                .withL4Balance(new RoundRobin())
                .withCluster(cluster)
                .withFrontListener(new TCPListener(new InetSocketAddress("0.0.0.0", 9110)))
                .build();

        l4LoadBalancer.start();*/

        HTTPConfiguration httpConfiguration = HTTPConfigurationBuilder.newBuilder()
                .withBrotliCompressionLevel(4)
                .withCompressionThreshold(100)
                .withDeflateCompressionLevel(6)
                .withH2enablePush(false)
                .withH2InitialWindowSize(65535)
                .withMaxChunkSize(1024 * 10)
                .withH2MaxConcurrentStreams(100)
                .withMaxContentLength(1024 * 1024)
                .withMaxHeaderSize(1024 * 10)
                .withH2MaxHeaderSizeList(4294967295L)
                .withMaxInitialLineLength(1024 * 10)
                .withH2MaxFrameSize(16777215)
                .withH2MaxHeaderTableSize(4096)
                .build();

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCommonConfiguration(configuration)
                .withL7Balance(new RoundRobin())
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("0.0.0.0", 9110))
                .withHTTPFrontListener(new HTTPListener(tlsConfiguration, forClient))
                .withHTTPConfiguration(httpConfiguration)
                .build();

        httpLoadBalancer.start();
    }
}
