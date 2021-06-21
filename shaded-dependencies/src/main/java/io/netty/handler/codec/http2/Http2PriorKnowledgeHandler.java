/*
 * Copyright 2021 The Netty Project
 *
 * The Netty Project licenses this file to you under the Apache License,
 * version 2.0 (the "License"); you may not use this file except in compliance
 * with the License. You may obtain a copy of the License at:
 *
 *   https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
 * WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
 * License for the specific language governing permissions and limitations
 * under the License.
 */
package io.netty.handler.codec.http2;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import io.netty.util.ReferenceCountUtil;

import static io.netty.buffer.Unpooled.unreleasableBuffer;
import static io.netty.handler.codec.http2.Http2CodecUtil.connectionPrefaceBuf;

/**
 * <p>
 * This class is used for holding HTTP/2 preface message
 * until {@link ApplicationProtocolNegotiationHandler} finishes
 * configuring pipeline. {@link ApplicationProtocolNegotiationHandler}
 * will automatically call {@link #releasePreface()} to release the preface message.
 * This class must be added before {@link ApplicationProtocolNegotiationHandler}.
 * </p>
 *
 * <p>
 * If this class is being outside of {@link ApplicationProtocolNegotiationHandler},
 * user must call {@link #releasePreface()} manually once they're done configuring pipeline
 * to receive the preface message.
 * </p>
 */
public final class Http2PriorKnowledgeHandler extends ChannelInboundHandlerAdapter {

    private static final ByteBuf CONNECTION_PREFACE = unreleasableBuffer(connectionPrefaceBuf());
    private ChannelHandlerContext ctx;
    private ByteBuf prefaceBuf;

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) throws Exception {
        this.ctx = ctx;
        super.handlerAdded(ctx);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        ByteBuf in = (ByteBuf) msg;
        int prefaceLength = CONNECTION_PREFACE.readableBytes();
        int bytesRead = Math.min(in.readableBytes(), prefaceLength);

        if (!ByteBufUtil.equals(CONNECTION_PREFACE, CONNECTION_PREFACE.readerIndex(), in, in.readerIndex(), bytesRead)) {
            // It was not HTTP/2 Preface, let's remove ourselves from the pipeline.
            ctx.pipeline().remove(this);
        } else if (bytesRead == prefaceLength) {
            // We got the Preface message, let's hold it for now.
            prefaceBuf = in;
            return;
        }

        ctx.fireChannelRead(msg);
    }

    /**
     * Release the {@link ByteBuf} holding preface message
     * into pipeline.
     */
    public void releasePreface() {
        ctx.pipeline().remove(this);
        ctx.fireChannelRead(prefaceBuf);
    }
}
