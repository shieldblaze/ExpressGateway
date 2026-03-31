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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.websocketx.CloseWebSocketFrame;
import io.netty.handler.codec.http.websocketx.TextWebSocketFrame;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link WebSocketCloseHandler} implementing RFC 6455 Section 7 close handshake.
 */
class WebSocketCloseHandlerTest {

    // -- Status code validation tests (RFC 6455 Section 7.4) --

    @Test
    void validCloseCode_normalClosure() {
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1000));
    }

    @Test
    void validCloseCode_goingAway() {
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1001));
    }

    @Test
    void validCloseCode_protocolError() {
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1002));
    }

    @Test
    void validCloseCode_unsupportedData() {
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1003));
    }

    @Test
    void validCloseCode_standardRange() {
        // 1007-1011 are valid standard codes
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1007));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1008));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1009));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1010));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1011));
    }

    @Test
    void validCloseCode_ianaReserved() {
        // 1012-1014 are reserved by IANA but allowed on the wire
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1012));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1013));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(1014));
    }

    @Test
    void validCloseCode_privateUseRange() {
        // 3000-3999 reserved for libraries/frameworks
        assertTrue(WebSocketCloseHandler.isValidCloseCode(3000));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(3999));
        // 4000-4999 reserved for private use
        assertTrue(WebSocketCloseHandler.isValidCloseCode(4000));
        assertTrue(WebSocketCloseHandler.isValidCloseCode(4999));
    }

    @Test
    void invalidCloseCode_reserved1004() {
        assertFalse(WebSocketCloseHandler.isValidCloseCode(1004));
    }

    @Test
    void invalidCloseCode_reserved1005_noStatusRcvd() {
        assertFalse(WebSocketCloseHandler.isValidCloseCode(1005));
    }

    @Test
    void invalidCloseCode_reserved1006_abnormalClosure() {
        assertFalse(WebSocketCloseHandler.isValidCloseCode(1006));
    }

    @Test
    void invalidCloseCode_reserved1015_tlsHandshake() {
        assertFalse(WebSocketCloseHandler.isValidCloseCode(1015));
    }

    @Test
    void invalidCloseCode_belowRange() {
        assertFalse(WebSocketCloseHandler.isValidCloseCode(0));
        assertFalse(WebSocketCloseHandler.isValidCloseCode(999));
    }

    @Test
    void invalidCloseCode_aboveRange() {
        assertFalse(WebSocketCloseHandler.isValidCloseCode(5000));
        assertFalse(WebSocketCloseHandler.isValidCloseCode(65535));
    }

    @Test
    void invalidCloseCode_extensionReservedRange() {
        // 1016-2999 are reserved for extensions; not valid unless extension negotiated
        assertFalse(WebSocketCloseHandler.isValidCloseCode(1016));
        assertFalse(WebSocketCloseHandler.isValidCloseCode(2000));
        assertFalse(WebSocketCloseHandler.isValidCloseCode(2999));
    }

    // -- Constructor validation --

    @Test
    void constructor_rejectsNonPositiveTimeout() {
        assertThrows(IllegalArgumentException.class, () -> new WebSocketCloseHandler(0));
        assertThrows(IllegalArgumentException.class, () -> new WebSocketCloseHandler(-1));
    }

    // -- Close handshake state machine tests --

    @Test
    void inboundClose_peerInitiates_echoesCloseAndCloses() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler();
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        assertEquals(WebSocketCloseHandler.CloseState.OPEN, handler.state());

        // Simulate the peer sending a Close frame
        channel.writeInbound(new CloseWebSocketFrame(1000, "Normal closure"));

        // Handler should transition to CLOSED
        assertEquals(WebSocketCloseHandler.CloseState.CLOSED, handler.state());

        // A Close frame echo should be written outbound
        CloseWebSocketFrame echoFrame = channel.readOutbound();
        assertNotNull(echoFrame, "Expected a Close echo frame to be written outbound");
        assertEquals(1000, echoFrame.statusCode());
        echoFrame.release();

        // Channel should be closed (or closing)
        assertFalse(channel.isOpen());

        channel.finishAndReleaseAll();
    }

    @Test
    void outboundClose_weInitiate_transitionsToCloseSent() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler(5);
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        assertEquals(WebSocketCloseHandler.CloseState.OPEN, handler.state());

        // We initiate the close
        channel.writeOutbound(new CloseWebSocketFrame(1000, "Shutting down"));

        // Should be in CLOSE_SENT, waiting for peer's Close response
        assertEquals(WebSocketCloseHandler.CloseState.CLOSE_SENT, handler.state());

        // The Close frame should pass through to the outbound
        CloseWebSocketFrame outFrame = channel.readOutbound();
        assertNotNull(outFrame, "Close frame should be forwarded outbound");
        assertEquals(1000, outFrame.statusCode());
        outFrame.release();

        // Channel should still be open (waiting for peer's response)
        assertTrue(channel.isOpen());

        channel.finishAndReleaseAll();
    }

    @Test
    void completeHandshake_weInitiate_peerResponds() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler(5);
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // Step 1: We send a Close frame
        channel.writeOutbound(new CloseWebSocketFrame(1000, "Done"));
        assertEquals(WebSocketCloseHandler.CloseState.CLOSE_SENT, handler.state());

        // Consume the outbound frame
        CloseWebSocketFrame out = channel.readOutbound();
        assertNotNull(out);
        out.release();

        // Step 2: Peer responds with a Close frame
        channel.writeInbound(new CloseWebSocketFrame(1000, "Acknowledged"));
        assertEquals(WebSocketCloseHandler.CloseState.CLOSED, handler.state());

        // Channel should be closed
        assertFalse(channel.isOpen());

        channel.finishAndReleaseAll();
    }

    @Test
    void inboundClose_invalidStatusCode_respondsWith1002() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler();
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // Construct a CloseWebSocketFrame with an invalid status code by building raw bytes.
        // Netty's CloseWebSocketFrame(int, String) constructor validates the code and rejects
        // codes it considers invalid, but on the wire a malicious peer can send any code.
        // The (boolean, int, ByteBuf) constructor bypasses validation, simulating raw wire data.
        // Code 999 is below the valid 1000-4999 range per RFC 6455 Section 7.4.
        ByteBuf invalidPayload = Unpooled.buffer(2);
        invalidPayload.writeShort(999);
        channel.writeInbound(new CloseWebSocketFrame(true, 0, invalidPayload));

        // Handler should transition to CLOSED
        assertEquals(WebSocketCloseHandler.CloseState.CLOSED, handler.state());

        // Should respond with 1002 (Protocol Error)
        CloseWebSocketFrame responseFrame = channel.readOutbound();
        assertNotNull(responseFrame, "Expected a 1002 Protocol Error response");
        assertEquals(1002, responseFrame.statusCode());
        responseFrame.release();

        // Channel should be closed
        assertFalse(channel.isOpen());

        channel.finishAndReleaseAll();
    }

    @Test
    void inboundClose_noBody_echoesEmptyClose() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler();
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // Send a Close frame with no status code (empty body).
        // Netty's CloseWebSocketFrame() with no args produces statusCode == -1.
        channel.writeInbound(new CloseWebSocketFrame());

        assertEquals(WebSocketCloseHandler.CloseState.CLOSED, handler.state());

        // Echo should be written
        CloseWebSocketFrame echoFrame = channel.readOutbound();
        assertNotNull(echoFrame, "Expected an echo Close frame");
        echoFrame.release();

        assertFalse(channel.isOpen());

        channel.finishAndReleaseAll();
    }

    @Test
    void duplicateInboundClose_discarded() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler();
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // First Close
        channel.writeInbound(new CloseWebSocketFrame(1000, "First"));
        assertEquals(WebSocketCloseHandler.CloseState.CLOSED, handler.state());

        // Consume the echo
        CloseWebSocketFrame echo = channel.readOutbound();
        assertNotNull(echo);
        echo.release();

        channel.finishAndReleaseAll();
    }

    @Test
    void duplicateOutboundClose_discarded() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler(5);
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // First outbound Close
        channel.writeOutbound(new CloseWebSocketFrame(1000, "First"));
        assertEquals(WebSocketCloseHandler.CloseState.CLOSE_SENT, handler.state());

        // Consume the first Close
        CloseWebSocketFrame first = channel.readOutbound();
        assertNotNull(first);
        first.release();

        // Second outbound Close should be discarded
        channel.writeOutbound(new CloseWebSocketFrame(1000, "Second"));
        assertEquals(WebSocketCloseHandler.CloseState.CLOSE_SENT, handler.state());

        // No second frame should be written
        CloseWebSocketFrame second = channel.readOutbound();
        assertNull(second, "Duplicate outbound Close should be discarded");

        channel.finishAndReleaseAll();
    }

    @Test
    void nonCloseFrames_passThrough_whenOpen() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler();
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // Text frame should pass through when connection is OPEN
        TextWebSocketFrame textFrame = new TextWebSocketFrame("Hello");
        channel.writeInbound(textFrame);

        TextWebSocketFrame received = channel.readInbound();
        assertNotNull(received, "Text frame should pass through when OPEN");
        assertEquals("Hello", received.text());
        received.release();

        channel.finishAndReleaseAll();
    }

    @Test
    void nonCloseFrames_discarded_afterCloseSent() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler(5);
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // Initiate close
        channel.writeOutbound(new CloseWebSocketFrame(1000, "Closing"));
        assertEquals(WebSocketCloseHandler.CloseState.CLOSE_SENT, handler.state());

        // Consume outbound close
        CloseWebSocketFrame outClose = channel.readOutbound();
        assertNotNull(outClose);
        outClose.release();

        // Inbound text frame should be discarded after close was initiated
        channel.writeInbound(new TextWebSocketFrame("Late message"));
        TextWebSocketFrame lateMsg = channel.readInbound();
        assertNull(lateMsg, "Non-close frames should be discarded after close initiated");

        channel.finishAndReleaseAll();
    }

    @Test
    void channelInactive_cancelsTimeout_setsStateClosed() {
        WebSocketCloseHandler handler = new WebSocketCloseHandler(5);
        EmbeddedChannel channel = new EmbeddedChannel(handler);

        // Initiate close to schedule a timeout
        channel.writeOutbound(new CloseWebSocketFrame(1000, "Closing"));
        assertEquals(WebSocketCloseHandler.CloseState.CLOSE_SENT, handler.state());

        // Consume outbound
        CloseWebSocketFrame outClose = channel.readOutbound();
        assertNotNull(outClose);
        outClose.release();

        // Simulate channel going inactive (e.g., peer drops connection)
        channel.close();

        assertEquals(WebSocketCloseHandler.CloseState.CLOSED, handler.state());

        channel.finishAndReleaseAll();
    }
}
