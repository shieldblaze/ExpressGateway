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
package com.shieldblaze.expressgateway.microbench.protocol.http;

import com.shieldblaze.expressgateway.protocol.http.Headers;
import com.shieldblaze.expressgateway.protocol.http.adapter.http1.HTTPOutboundAdapter;
import com.shieldblaze.expressgateway.protocol.http.adapter.http2.HTTP2InboundAdapter;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.Test;
import org.openjdk.jmh.annotations.Benchmark;
import org.openjdk.jmh.annotations.BenchmarkMode;
import org.openjdk.jmh.annotations.Mode;
import org.openjdk.jmh.annotations.Scope;
import org.openjdk.jmh.annotations.Setup;
import org.openjdk.jmh.annotations.State;
import org.openjdk.jmh.annotations.Threads;
import org.openjdk.jmh.annotations.Warmup;
import org.openjdk.jmh.infra.Blackhole;
import org.openjdk.jmh.runner.Runner;
import org.openjdk.jmh.runner.RunnerException;
import org.openjdk.jmh.runner.options.Options;
import org.openjdk.jmh.runner.options.OptionsBuilder;

@State(Scope.Benchmark)
@BenchmarkMode({Mode.Throughput, Mode.AverageTime})
@Warmup(iterations = 2)
@Threads(1)
public class HTTP2ToHTTPAdapterBenchTest {

    private static final Logger logger = LogManager.getLogger(HTTP2ToHTTPAdapterBenchTest.class);

    private EmbeddedChannel inboundChannel;
    private EmbeddedChannel outboundChannel;

    @Test
    void runBenchmark() throws RunnerException {
        Options opt = new OptionsBuilder()
                .include(HTTP2ToHTTPAdapterBenchTest.class.getSimpleName())
                .forks(5)
                .addProfiler("gc")
                .build();

        new Runner(opt).run();
        if (System.getProperty("performBench") != null && Boolean.parseBoolean(System.getProperty("performBench"))) {
   /*         Options opt = new OptionsBuilder()
                    .include(HTTP2ToHTTPAdapterBenchTest.class.getSimpleName())
                    .forks(5)
                    .addProfiler("gc")
                    .build();

            new Runner(opt).run();*/
        } else {
            logger.info("\"performBench\" is set to false, skipping benchmarking test.");
        }
    }

    @Setup
    public void setup() {
        inboundChannel = new EmbeddedChannel(new HTTP2InboundAdapter(), new InboundHandler(this));
        outboundChannel = new EmbeddedChannel(new OutboundHandler(this), new HTTPOutboundAdapter());
    }

    @Benchmark
    public void benchmark(Blackhole blackhole) {
        Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), true);
        http2HeadersFrame.headers().method("GET")
                .path("/")
                .scheme("https");
        http2HeadersFrame.stream(new CustomHttp2FrameStream(2));

        inboundChannel.writeInbound(http2HeadersFrame);
        inboundChannel.flushInbound();

        Http2HeadersFrame headerFrame = inboundChannel.readOutbound();
        Http2DataFrame dataFrame = inboundChannel.readOutbound();
        dataFrame.release();

        blackhole.consume(headerFrame);
        blackhole.consume(dataFrame);
    }

    private static final class InboundHandler extends ChannelInboundHandlerAdapter {

        private final HTTP2ToHTTPAdapterBenchTest benchTest;

        InboundHandler(HTTP2ToHTTPAdapterBenchTest benchTest) {
            this.benchTest = benchTest;
        }

        @Override
        public void channelRead(ChannelHandlerContext ctx, Object msg) {
            benchTest.outboundChannel.writeInbound(msg);
            benchTest.outboundChannel.flushInbound();
        }
    }

    private static final class OutboundHandler extends ChannelInboundHandlerAdapter {

        private final HTTP2ToHTTPAdapterBenchTest benchTest;

        OutboundHandler(HTTP2ToHTTPAdapterBenchTest benchTest) {
            this.benchTest = benchTest;
        }

        @Override
        public void channelRead(ChannelHandlerContext ctx, Object msg) {
            FullHttpRequest httpRequest = (FullHttpRequest) msg;
            HttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.wrappedBuffer("GG".getBytes()));
            httpResponse.headers().set(Headers.STREAM_HASH, httpRequest.headers().get(Headers.STREAM_HASH));
            benchTest.inboundChannel.writeOutbound(httpResponse);
            benchTest.inboundChannel.flushOutbound();
            httpRequest.release();
        }
    }
}
