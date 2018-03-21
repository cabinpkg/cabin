module TestCases.Lazy where

import VirtualDom exposing (Node, lazy)
import ElmTest.Assertion exposing (assertEqual)
import ElmTest.Test exposing (Test, suite, test)

import TestHelpers exposing (renderDom, updateDom, unsafeRecordCallCount, unsafeQueryCallCount)

renderRecord : { x: String, y: String } -> Node
renderRecord r =
    VirtualDom.text <| "The values: " ++ r.x ++ ", " ++ r.y


renderPrimitive : Int -> Node
renderPrimitive x =
    VirtualDom.text <| "The value: " ++ (toString x)


testLazyIdenticalRecord =
    test "isn't called again with identical record" <|
        let record = { x = "a", y = "b" }
            wrappedRender = unsafeRecordCallCount renderRecord
            v1 = renderDom <| lazy wrappedRender record
            v2 = updateDom v1 <| lazy wrappedRender record
            v3 = updateDom v2 <| lazy wrappedRender record
        in
            assertEqual 1 <| unsafeQueryCallCount wrappedRender


testLazyIdenticalPrimitive =
    test "isn't called again with identical primitive" <|
        let wrappedRender = unsafeRecordCallCount renderPrimitive
            v1 = renderDom <| lazy wrappedRender 5
            v2 = updateDom v1 <| lazy wrappedRender 5
            v3 = updateDom v2 <| lazy wrappedRender 5
        in
            assertEqual 1 <| unsafeQueryCallCount wrappedRender


testLazyRecordMutationOfIdenticalValue =
    test "isn't called again with record mutation of identical value" <|
        let record = { x = "a", y = "b" }
            wrappedRender = unsafeRecordCallCount renderRecord
            v1 = renderDom <| lazy wrappedRender record
            v2 = updateDom v1 <| lazy wrappedRender { record | x = "a" }
            v3 = updateDom v2 <| lazy wrappedRender { record | x = "a", y = "b" }
        in
            assertEqual 1 <| unsafeQueryCallCount wrappedRender


testNotLazyDifferentRecord =
    test "is called again with an equivalent but different record" <|
        let wrappedRender = unsafeRecordCallCount renderRecord
            v1 = renderDom <| lazy wrappedRender { x = "a", y = "b" }
            v2 = updateDom v1 <| lazy wrappedRender { x = "a", y = "b" }
            v3 = updateDom v2 <| lazy wrappedRender { x = "a", y = "b" }
        in
            assertEqual 3 <| unsafeQueryCallCount wrappedRender


tests : Test
tests =
    suite
        "Lazy"
        [
            testLazyIdenticalRecord,
            testLazyIdenticalPrimitive,
            -- Re-enable this test when core supports checking
            -- record update values for identity before copying:
            -- testLazyRecordMutationOfIdenticalValue,
            testNotLazyDifferentRecord
        ]
