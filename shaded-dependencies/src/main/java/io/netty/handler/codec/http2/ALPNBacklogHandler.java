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

import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import io.netty.util.ReferenceCountUtil;
import io.netty.util.internal.RecyclableArrayList;

/**
 * This class is used to hold messages such as HTTP/2 Preface (in Prior Knowledge mode)
 * during ALPN. Once ALPN is completed, all messages are fired back into the pipeline.
 */
public final class ALPNBacklogHandler extends ApplicationProtocolNegotiationHandler.PipelineCallback {

    private final RecyclableArrayList backlogList = RecyclableArrayList.newInstance();
    private ChannelHandlerContext ctx;

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) throws Exception {
        this.ctx = ctx;
        super.handlerAdded(ctx);
    }

    @Override
    public void handlerRemoved(ChannelHandlerContext ctx) throws Exception {
        backlogList.recycle();
        super.handlerRemoved(ctx);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        backlogList.add(msg);
    }

    @Override
    public void success() {
        for (Object o : backlogList) {
            ctx.fireChannelRead(o);
        }
        ctx.pipeline().remove(this);
    }

    @Override
    public void failure() {
        for (Object o : backlogList) {
            ReferenceCountUtil.release(o);
        }
        ctx.pipeline().remove(this);
    }
}
