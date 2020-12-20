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
package com.shieldblaze.expressgateway.protocol.http;

import com.aayushatharva.brotli4j.Brotli4jLoader;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.MutualTLS;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerMapping;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.handler.ssl.util.SelfSignedCertificate;

import javax.net.ssl.SSLException;
import java.net.InetSocketAddress;
import java.security.cert.CertificateException;
import java.util.Arrays;
import java.util.Collections;

public final class Main {

    static {
        Brotli4jLoader.isAvailable();
        System.setProperty("log4j.configurationFile", "log4j2.xml");
//        System.loadLibrary("netty_tcnative");
    }

    public static void main(String[] args) throws CertificateException, SSLException {

        TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.EPOLL)
                .withTCPFastOpenMaximumPendingRequests(2147483647)
                .withBackendConnectTimeout(10000 * 5)
                .withBackendSocketTimeout(10000 * 5)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{65535})
                .withSocketReceiveBufferSize(2147483647)
                .withSocketSendBufferSize(2147483647)
                .withTCPConnectionBacklog(2147483647)
                .withDataBacklog(2147483647)
                .withConnectionIdleTimeout(1800000)
                .build();

        EventLoopConfiguration eventLoopConfiguration = EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(32)
                .withChildWorkers(32)
                .build();

        CoreConfiguration configuration = CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration.DEFAULT)
                .build();

        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(
                Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key(), false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);

        TLSConfiguration forServer = TLSConfigurationBuilder.forServer()
                .withProtocols(Arrays.asList(Protocol.TLS_1_3, Protocol.TLS_1_2))
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_128_GCM_SHA256))
                .withUseALPN(true)
                .withTLSServerMapping(tlsServerMapping)
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .build();

        TLSConfiguration forClient = TLSConfigurationBuilder.forClient()
                .withProtocols(Arrays.asList(Protocol.TLS_1_3, Protocol.TLS_1_2))
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withUseALPN(true)
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withTrustManager(InsecureTrustManagerFactory.INSTANCE.getTrustManagers()[0])
                .build();

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 8080));

        HTTPConfiguration httpConfiguration = HTTPConfigurationBuilder.newBuilder()
                .withBrotliCompressionLevel(4)
                .withCompressionThreshold(100)
                .withDeflateCompressionLevel(6)
                .withMaxChunkSize(1024 * 100)
                .withMaxContentLength(1024 * 10240)
                .withMaxHeaderSize(1024 * 10)
                .withMaxInitialLineLength(1024 * 100)
                .withH2enablePush(false)
                .withH2InitialWindowSize(Integer.MAX_VALUE)
                .withH2MaxConcurrentStreams(1000)
                .withH2MaxHeaderSizeList(262144)
                .withH2MaxFrameSize(16777215)
                .withH2MaxHeaderTableSize(65536)
                .build();

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(configuration)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 9110))
                .withHTTPConfiguration(httpConfiguration)
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.start();
    }
}
