module VirtualDom.Helpers exposing
  ( Node
  , text, node, div, span, a, h1
  , Property, property, attribute
  , class, id, href
  , style
  , on, onWithOptions, Options, defaultOptions
  , onClick
  , map
  , lazy, lazy2, lazy3
  , keyedNode
  )


import Json.Decode as Decode
import Json.Encode as Encode
import Native.VirtualDom


type Node msg = Node


node : String -> List (Property msg) -> List (Node msg) -> Node msg
node =
  Native.VirtualDom.node


text : String -> Node msg
text =
  Native.VirtualDom.text


div : List (Property msg) -> List (Node msg) -> Node msg
div =
  node "div"


span : List (Property msg) -> List (Node msg) -> Node msg
span =
  node "span"


a : List (Property msg) -> List (Node msg) -> Node msg
a =
  node "a"


h1 : List (Property msg) -> List (Node msg) -> Node msg
h1 =
  node "h1"


map : (a -> msg) -> Node a -> Node msg
map =
  Native.VirtualDom.map


type Property msg = Property


property : String -> Decode.Value -> Property msg
property =
  Native.VirtualDom.property


attribute : String -> String -> Property msg
attribute =
  Native.VirtualDom.attribute


class : String -> Property msg
class name =
  property "className" (Encode.string name)


href : String -> Property msg
href name =
  property "href" (Encode.string name)


id : String -> Property msg
id =
  attribute "id"


style : List (String, String) -> Property msg
style =
  Native.VirtualDom.style


on : String -> Decode.Decoder msg -> Property msg
on eventName decoder =
  onWithOptions eventName defaultOptions decoder


onClick : msg -> Property msg
onClick msg =
  on "click" (Decode.succeed msg)


onWithOptions : String -> Options -> Decode.Decoder msg -> Property msg
onWithOptions =
  Native.VirtualDom.on


type alias Options =
  { stopPropagation : Bool
  , preventDefault : Bool
  }


defaultOptions : Options
defaultOptions =
  { stopPropagation = False
  , preventDefault = False
  }


lazy : (a -> Node msg) -> a -> Node msg
lazy =
  Native.VirtualDom.lazy


lazy2 : (a -> b -> Node msg) -> a -> b -> Node msg
lazy2 =
  Native.VirtualDom.lazy2


lazy3 : (a -> b -> c -> Node msg) -> a -> b -> c -> Node msg
lazy3 =
  Native.VirtualDom.lazy3


keyedNode : String -> List (Property msg) -> List ( String, Node msg ) -> Node msg
keyedNode =
  Native.VirtualDom.keyedNode

