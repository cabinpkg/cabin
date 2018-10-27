module TestHelpers where

import VirtualDom exposing (Node)

import Native.TestHelpers
import Native.VirtualDom

unsafeRecordCallCount : (a -> b) -> (a -> b)
unsafeRecordCallCount =
    Native.TestHelpers.unsafeRecordCallCount

unsafeQueryCallCount : (a -> b) -> Int
unsafeQueryCallCount =
    Native.TestHelpers.unsafeQueryCallCount

type OpaqueDom = OpaqueDom

render : Node -> OpaqueDom
render =
    Native.VirtualDom.render

updateAndReplace : OpaqueDom -> Node -> Node -> OpaqueDom
updateAndReplace =
    Native.TestHelpers.updateAndReplace


renderDom : Node -> (OpaqueDom, Node)
renderDom vdom =
    (render vdom, vdom)


updateDom : (OpaqueDom, Node) -> Node -> (OpaqueDom, Node)
updateDom (oldDom, oldVDom) newVDom =
    (updateAndReplace oldDom oldVDom newVDom, newVDom)
