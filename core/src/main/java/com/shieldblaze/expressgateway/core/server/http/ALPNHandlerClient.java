package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
import io.netty.handler.logging.LogLevel;
import io.netty.handler.logging.LoggingHandler;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import io.netty.util.concurrent.Promise;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class ALPNHandlerClient extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNHandlerClient.class);

    private final HTTPConfiguration httpConfiguration;
    private final DownstreamHandler downstreamHandler;
    private final Promise<Void> promise;

    ALPNHandlerClient(HTTPConfiguration httpConfiguration, DownstreamHandler downstreamHandler, Promise<Void> promise) {
        super(ApplicationProtocolNames.HTTP_1_1);
        this.httpConfiguration = httpConfiguration;
        this.downstreamHandler = downstreamHandler;
        this.promise = promise;
    }

    @Override
    protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
            ctx.pipeline().addLast(
                    Http2FrameCodecBuilder.forClient()
                            .autoAckPingFrame(true)
                            .autoAckSettingsFrame(true)
                            .validateHeaders(true)
                            .build(),
                    new LoggingHandler(LogLevel.DEBUG),
                    new HTTP2ClientTranslationHandler(),
                    downstreamHandler
            );
            promise.trySuccess(null);
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
            ctx.pipeline().addLast(
                    new HttpClientCodec(httpConfiguration.getMaxInitialLineLength(), httpConfiguration.getMaxHeaderSize(),
                            httpConfiguration.getMaxChunkSize(), true, true),
                    downstreamHandler
            );
            promise.trySuccess(null);
        } else {
            logger.error("Unsupported ALPN Protocol: {}", protocol);
            ctx.channel().closeFuture();
            promise.tryFailure(new IllegalArgumentException("Unsupported Protocol: " +  protocol));
        }
    }


    Promise<Void> promise() {
        return promise;
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
//        logger.error("Caught Error at ALPN Client Handler", cause);
    }
}
