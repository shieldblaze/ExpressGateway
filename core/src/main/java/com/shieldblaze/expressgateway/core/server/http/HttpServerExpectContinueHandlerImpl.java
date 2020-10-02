package com.shieldblaze.expressgateway.core.server.http;

import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpMessage;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpServerExpectContinueHandler;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.util.ReferenceCountUtil;

import static io.netty.handler.codec.http.HttpUtil.getContentLength;

/**
 * Validate Content-Length and Expect Header
 */
public class HttpServerExpectContinueHandlerImpl extends ChannelInboundHandlerAdapter {

    private final int maxContentLength;

    public HttpServerExpectContinueHandlerImpl(int maxContentLength) {
        this.maxContentLength = maxContentLength;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;

            if (getContentLength(request, -1L) > maxContentLength) {
                ctx.writeAndFlush(HttpResponses.TOO_LARGE.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                ReferenceCountUtil.release(msg);
                return;
            }

            if (isUnsupportedExpectation(request)) {
                ctx.writeAndFlush(HttpResponses.EXPECTATION_FAILED.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                ReferenceCountUtil.release(msg);
                return;
            }

            if (HttpUtil.is100ContinueExpected(request)) {
                ctx.writeAndFlush(HttpResponses.ACCEPT_KEEP_ALIVE.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                request.headers().remove(HttpHeaderNames.EXPECT);
            }
        }
        super.channelRead(ctx, msg);
    }

    static boolean isUnsupportedExpectation(HttpMessage message) {
        if (!isExpectHeaderValid(message)) {
            return false;
        }

        String expectValue = message.headers().get(HttpHeaderNames.EXPECT);
        return expectValue != null && !HttpHeaderValues.CONTINUE.toString().equalsIgnoreCase(expectValue);
    }

    private static boolean isExpectHeaderValid(final HttpMessage message) {
        /*
         * Expect: 100-continue is for requests only and it works only on HTTP/1.1 or later. Note further that RFC 7231
         * section 5.1.1 says "A server that receives a 100-continue expectation in an HTTP/1.0 request MUST ignore
         * that expectation."
         */
        return message instanceof HttpRequest && message.protocolVersion().compareTo(HttpVersion.HTTP_1_1) >= 0;
    }
}
